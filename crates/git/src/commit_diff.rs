//! What a past commit changed: its files and the diff of each, for the
//! history browser's detail view. Read-only; uses libgit2 only.

use std::path::Path;

use git2::{Delta, DiffFindOptions, Oid, Repository};

use crate::diff::blob_diff;
use crate::{check_rel_path, ChangeKind, FileDiff, GitError};

/// A commit with more changed files than this lists the first ones only.
const MAX_FILES: usize = 1000;

/// One file a commit changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    /// Previous path when the commit renamed the file.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
}

/// Everything about a commit's changes: its files (sorted by path) and
/// whether the list was cut short.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitChanges {
    pub files: Vec<CommitFile>,
    pub truncated: bool,
}

fn diff_of<'r>(repo: &'r Repository, hash: &str) -> Result<git2::Diff<'r>, GitError> {
    let oid = Oid::from_str(hash).map_err(|_| GitError::Invalid("not a commit id".into()))?;
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;
    // A merge is shown against its first parent, like `git show --first-parent`.
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    let mut find = DiffFindOptions::new();
    find.renames(true);
    diff.find_similar(Some(&mut find))?;
    Ok(diff)
}

fn kind_of(status: Delta) -> ChangeKind {
    match status {
        Delta::Added | Delta::Copied => ChangeKind::Added,
        Delta::Deleted => ChangeKind::Deleted,
        Delta::Renamed => ChangeKind::Renamed,
        Delta::Typechange => ChangeKind::Typechange,
        Delta::Conflicted => ChangeKind::Conflicted,
        _ => ChangeKind::Modified,
    }
}

/// The files `hash` changed.
pub fn commit_changes(path: &Path, hash: &str) -> Result<CommitChanges, GitError> {
    let repo = Repository::open(path)?;
    let diff = diff_of(&repo, hash)?;
    let mut files: Vec<CommitFile> = Vec::new();
    let mut truncated = false;
    for delta in diff.deltas() {
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        let status = delta.status();
        let Some(shown) = (if status == Delta::Deleted {
            old_path.clone()
        } else {
            new_path.clone()
        }) else {
            continue;
        };
        files.push(CommitFile {
            old_path: (status == Delta::Renamed)
                .then(|| old_path.clone())
                .flatten(),
            path: shown.replace('\\', "/"),
            kind: kind_of(status),
        });
    }
    files.sort_by_key(|f| f.path.to_lowercase());
    Ok(CommitChanges { files, truncated })
}

/// The diff of one file of commit `hash`.
pub fn commit_file_diff(path: &Path, hash: &str, file: &str) -> Result<FileDiff, GitError> {
    check_rel_path(file)?;
    let repo = Repository::open(path)?;
    let diff = diff_of(&repo, hash)?;
    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        let matches = match delta.status() {
            Delta::Deleted => old_path.as_deref() == Some(file),
            _ => new_path.as_deref() == Some(file),
        };
        if !matches {
            continue;
        }
        let side = |id: Oid| (!id.is_zero()).then_some(id);
        let mut out = blob_diff(
            &repo,
            side(delta.old_file().id()),
            side(delta.new_file().id()),
            file,
        )?;
        if delta.flags().is_binary() || delta.new_file().is_binary() || delta.old_file().is_binary()
        {
            out.binary = true;
            out.hunks.clear();
        }
        return Ok(out);
    }
    Err(GitError::Invalid(format!(
        "{file} isn't part of this commit"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;
    use crate::LineKind;

    #[test]
    fn a_commit_lists_its_files_and_diffs_each() {
        let f = Fixture::with_files(&[("keep.txt", "1\n2\n3\n"), ("gone.txt", "bye\n")]);
        f.write("keep.txt", "1\nTWO\n3\n");
        f.write("new/added.txt", "hello\n");
        std::fs::remove_file(f.path().join("gone.txt")).unwrap();
        f.stage_all();
        let hash = f.commit("change things").to_string();

        let changes = commit_changes(f.path(), &hash).unwrap();
        let listed: Vec<(&str, ChangeKind)> = changes
            .files
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            listed,
            [
                ("gone.txt", ChangeKind::Deleted),
                ("keep.txt", ChangeKind::Modified),
                ("new/added.txt", ChangeKind::Added),
            ]
        );

        let diff = commit_file_diff(f.path(), &hash, "keep.txt").unwrap();
        let kinds: Vec<LineKind> = diff.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&LineKind::Removed) && kinds.contains(&LineKind::Added));
        let added = commit_file_diff(f.path(), &hash, "new/added.txt").unwrap();
        assert!(added.old_missing);
        let gone = commit_file_diff(f.path(), &hash, "gone.txt").unwrap();
        assert!(gone.new_missing);
    }

    #[test]
    fn the_first_commit_shows_everything_as_added_and_bad_input_is_refused() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        let root = crate::recent_commits(f.path(), 1).unwrap().remove(0).hash;
        let changes = commit_changes(f.path(), &root).unwrap();
        assert_eq!(changes.files.len(), 1);
        assert_eq!(changes.files[0].kind, ChangeKind::Added);
        assert!(commit_changes(f.path(), "--evil").is_err());
        assert!(commit_file_diff(f.path(), &root, "../outside").is_err());
        assert!(commit_file_diff(f.path(), &root, "nope.txt").is_err());
    }

    #[test]
    fn renames_are_recognised() {
        let f = Fixture::with_files(&[("old.txt", "a long enough line of text\nsecond\nthird\n")]);
        std::fs::rename(f.path().join("old.txt"), f.path().join("new.txt")).unwrap();
        f.stage_all();
        let hash = f.commit("rename").to_string();
        let changes = commit_changes(f.path(), &hash).unwrap();
        assert_eq!(changes.files.len(), 1);
        assert_eq!(changes.files[0].kind, ChangeKind::Renamed);
        assert_eq!(changes.files[0].old_path.as_deref(), Some("old.txt"));
    }
}
