//! Working-copy status and whole-file staging: the data behind the Changes
//! view. Hunk and line staging live in `diff.rs`.

use std::path::Path;

use git2::{BranchType, ErrorCode, Repository, Status, StatusOptions};

use crate::{check_rel_path, GitError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Typechange,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// One-letter badge, GitHub Desktop style.
    pub fn letter(self) -> &'static str {
        match self {
            ChangeKind::Added | ChangeKind::Untracked => "A",
            ChangeKind::Modified | ChangeKind::Typechange => "M",
            ChangeKind::Deleted => "D",
            ChangeKind::Renamed => "R",
            ChangeKind::Conflicted => "!",
        }
    }
}

/// One path with changes. `staged` is HEAD → index, `unstaged` is
/// index → working tree; either or both can be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// Previous path when the change is a rename.
    pub old_path: Option<String>,
    pub staged: Option<ChangeKind>,
    pub unstaged: Option<ChangeKind>,
}

impl ChangedFile {
    pub fn is_conflicted(&self) -> bool {
        self.unstaged == Some(ChangeKind::Conflicted)
    }

    /// Everything about this file is in the index (the checkbox is ticked).
    pub fn is_fully_staged(&self) -> bool {
        self.staged.is_some() && self.unstaged.is_none()
    }

    /// Part of it is staged and part is not (the checkbox is "mixed").
    pub fn is_partially_staged(&self) -> bool {
        self.staged.is_some() && self.unstaged.is_some()
    }

    /// The kind to badge the row with: the working-tree change if there is
    /// one, otherwise the staged one.
    pub fn display_kind(&self) -> ChangeKind {
        self.unstaged
            .or(self.staged)
            .unwrap_or(ChangeKind::Modified)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Checked-out branch; `None` when detached.
    pub branch: Option<String>,
    /// Short hash of HEAD when detached.
    pub detached_at: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// "merge", "rebase", "cherry-pick"... while an operation is unfinished.
    pub in_progress: Option<&'static str>,
    /// The repo has no commits yet.
    pub unborn: bool,
    /// Sorted by path, case-insensitively.
    pub files: Vec<ChangedFile>,
}

impl RepoStatus {
    pub fn staged_count(&self) -> usize {
        self.files.iter().filter(|f| f.staged.is_some()).count()
    }

    pub fn conflict_count(&self) -> usize {
        self.files.iter().filter(|f| f.is_conflicted()).count()
    }
}

fn kind_of_index(s: Status) -> Option<ChangeKind> {
    if s.contains(Status::INDEX_NEW) {
        Some(ChangeKind::Added)
    } else if s.contains(Status::INDEX_RENAMED) {
        Some(ChangeKind::Renamed)
    } else if s.contains(Status::INDEX_DELETED) {
        Some(ChangeKind::Deleted)
    } else if s.contains(Status::INDEX_TYPECHANGE) {
        Some(ChangeKind::Typechange)
    } else if s.contains(Status::INDEX_MODIFIED) {
        Some(ChangeKind::Modified)
    } else {
        None
    }
}

fn kind_of_workdir(s: Status) -> Option<ChangeKind> {
    if s.contains(Status::CONFLICTED) {
        Some(ChangeKind::Conflicted)
    } else if s.contains(Status::WT_NEW) {
        Some(ChangeKind::Untracked)
    } else if s.contains(Status::WT_RENAMED) {
        Some(ChangeKind::Renamed)
    } else if s.contains(Status::WT_DELETED) {
        Some(ChangeKind::Deleted)
    } else if s.contains(Status::WT_TYPECHANGE) {
        Some(ChangeKind::Typechange)
    } else if s.contains(Status::WT_MODIFIED) {
        Some(ChangeKind::Modified)
    } else {
        None
    }
}

fn state_label(state: git2::RepositoryState) -> Option<&'static str> {
    use git2::RepositoryState as S;
    match state {
        S::Clean => None,
        S::Merge => Some("merge"),
        S::Revert | S::RevertSequence => Some("revert"),
        S::CherryPick | S::CherryPickSequence => Some("cherry-pick"),
        S::Bisect => Some("bisect"),
        S::Rebase | S::RebaseInteractive | S::RebaseMerge => Some("rebase"),
        S::ApplyMailbox | S::ApplyMailboxOrRebase => Some("apply"),
    }
}

/// Branch, upstream distance and every changed path of the repo at `path`.
pub fn repo_status(path: &Path) -> Result<RepoStatus, GitError> {
    let repo = Repository::open(path)?;
    let mut status = RepoStatus {
        in_progress: state_label(repo.state()),
        ..RepoStatus::default()
    };

    // HEAD can be unborn (no commits) or detached; read the symbolic ref so
    // both cases still name the branch.
    let head_ref = repo.find_reference("HEAD")?;
    match head_ref.symbolic_target() {
        Some(target) => {
            let name = target.strip_prefix("refs/heads/").unwrap_or(target);
            status.branch = Some(name.to_owned());
            status.unborn = repo.find_reference(target).is_err();
        }
        None => {
            status.detached_at = head_ref
                .peel_to_commit()
                .ok()
                .map(|c| c.id().to_string()[..7].to_owned());
        }
    }

    if let Some(name) = &status.branch {
        if let Ok(branch) = repo.find_branch(name, BranchType::Local) {
            if let Ok(up) = branch.upstream() {
                status.upstream = up.name().ok().flatten().map(str::to_owned);
                if let (Some(local), Some(remote)) =
                    (branch.get().target(), up.get().target())
                {
                    if let Ok((ahead, behind)) = repo.graph_ahead_behind(local, remote) {
                        status.ahead = ahead;
                        status.behind = behind;
                    }
                }
            }
        }
    }

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true)
        .include_ignored(false);
    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        // A repo with no commits and no files can trip the status walk.
        Err(e) if e.code() == ErrorCode::UnbornBranch => return Ok(status),
        Err(e) => return Err(e.into()),
    };
    for entry in statuses.iter() {
        let flags = entry.status();
        if flags.contains(Status::IGNORED) || flags.is_empty() {
            continue;
        }
        let Some(reported) = entry.path() else { continue };
        // For a rename git2's `path()` is the *old* name; the file as it is
        // now is the new side of the rename delta.
        let renamed_to = if flags.contains(Status::INDEX_RENAMED) {
            entry.head_to_index()
        } else if flags.contains(Status::WT_RENAMED) {
            entry.index_to_workdir()
        } else {
            None
        }
        .and_then(|d| d.new_file().path())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|new| new != reported);
        let (path, old_path) = match renamed_to {
            Some(new) => (new, Some(reported.to_owned())),
            None => (reported.to_owned(), None),
        };
        status.files.push(ChangedFile {
            path,
            old_path,
            staged: kind_of_index(flags),
            unstaged: kind_of_workdir(flags),
        });
    }
    status
        .files
        .sort_by_cached_key(|f| f.path.to_lowercase());
    Ok(status)
}

/// Puts the working-tree state of each path into the index: additions and
/// edits are added, files that no longer exist are removed from it.
pub fn stage_files(path: &Path, files: &[String]) -> Result<(), GitError> {
    let repo = Repository::open(path)?;
    let mut index = repo.index()?;
    for file in files {
        let rel = Path::new(check_rel_path(file)?);
        if path.join(rel).symlink_metadata().is_ok() {
            index.add_path(rel)?;
        } else if index.get_path(rel, 0).is_some() {
            index.remove_path(rel)?;
        }
    }
    index.write()?;
    Ok(())
}

/// Resets the index entry of each path back to HEAD, keeping the working
/// tree as it is.
pub fn unstage_files(path: &Path, files: &[String]) -> Result<(), GitError> {
    let repo = Repository::open(path)?;
    for file in files {
        check_rel_path(file)?;
    }
    match repo.head().and_then(|h| h.peel(git2::ObjectType::Commit)) {
        Ok(head) => repo.reset_default(Some(&head), files.iter().map(String::as_str))?,
        // No commits yet: unstaging is just forgetting the entry.
        Err(e) if e.code() == ErrorCode::UnbornBranch => {
            let mut index = repo.index()?;
            for file in files {
                // Missing from the index already is fine.
                let _ = index.remove_path(Path::new(file));
            }
            index.write()?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Throws away all changes (staged and not) to each path: tracked files go
/// back to their HEAD content, new files are deleted. Before anything is
/// touched, the current content is copied to
/// `.git/devspace-discarded/<unix time>/<path>` so a mistaken discard can be
/// recovered (CLAUDE.md: prefer recoverable destructive actions). Returns the
/// backup directory, or `None` when there was nothing to copy.
pub fn discard_files(
    path: &Path,
    files: &[String],
) -> Result<Option<std::path::PathBuf>, GitError> {
    let repo = Repository::open(path)?;
    for file in files {
        check_rel_path(file)?;
    }
    let backup = crate::diff::backup_files(&repo, files)?;

    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut tracked = Vec::new();
    let mut index = repo.index()?;
    for file in files {
        let rel = Path::new(file);
        let in_head = head_tree
            .as_ref()
            .map(|t| t.get_path(rel).is_ok())
            .unwrap_or(false);
        if in_head {
            tracked.push(file.as_str());
        } else {
            // New to the repo: forget the index entry and delete the file.
            let _ = index.remove_path(rel);
            let full = path.join(rel);
            match full.symlink_metadata() {
                Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&full)?,
                Ok(_) => std::fs::remove_file(&full)?,
                Err(_) => {}
            }
        }
    }
    index.write()?;
    if !tracked.is_empty() {
        let mut co = git2::build::CheckoutBuilder::new();
        co.force();
        for file in &tracked {
            co.path(file);
        }
        repo.checkout_head(Some(&mut co))?;
    }
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;

    #[test]
    fn status_reports_staged_unstaged_and_untracked() {
        let f = Fixture::with_files(&[("a.txt", "one\n"), ("b.txt", "two\n")]);
        f.write("a.txt", "one changed\n");
        f.write("b.txt", "two changed\n");
        f.write("new.txt", "fresh\n");
        stage_files(f.path(), &["b.txt".into()]).unwrap();

        let st = repo_status(f.path()).unwrap();
        let by = |p: &str| st.files.iter().find(|x| x.path == p).unwrap().clone();
        assert_eq!(by("a.txt").unstaged, Some(ChangeKind::Modified));
        assert_eq!(by("a.txt").staged, None);
        assert!(by("b.txt").is_fully_staged());
        assert_eq!(by("new.txt").unstaged, Some(ChangeKind::Untracked));
        assert_eq!(st.staged_count(), 1);
        assert!(st.branch.is_some());
    }

    #[test]
    fn partially_staged_file_is_both() {
        let f = Fixture::with_files(&[("a.txt", "1\n2\n3\n")]);
        f.write("a.txt", "1\n2 staged\n3\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        f.write("a.txt", "1\n2 staged\n3 more\n");
        let st = repo_status(f.path()).unwrap();
        assert!(st.files[0].is_partially_staged());
    }

    #[test]
    fn status_on_a_repo_without_commits() {
        let f = Fixture::empty();
        f.write("x.txt", "x\n");
        let st = repo_status(f.path()).unwrap();
        assert!(st.unborn);
        assert_eq!(st.files.len(), 1);
        stage_files(f.path(), &["x.txt".into()]).unwrap();
        assert_eq!(repo_status(f.path()).unwrap().files[0].staged, Some(ChangeKind::Added));
        unstage_files(f.path(), &["x.txt".into()]).unwrap();
        assert_eq!(repo_status(f.path()).unwrap().files[0].staged, None);
    }

    #[test]
    fn stage_removes_deleted_files_and_unstage_restores() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        std::fs::remove_file(f.path().join("a.txt")).unwrap();
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let st = repo_status(f.path()).unwrap();
        assert_eq!(st.files[0].staged, Some(ChangeKind::Deleted));
        unstage_files(f.path(), &["a.txt".into()]).unwrap();
        let st = repo_status(f.path()).unwrap();
        assert_eq!(st.files[0].staged, None);
        assert_eq!(st.files[0].unstaged, Some(ChangeKind::Deleted));
    }

    #[test]
    fn untracked_directories_list_their_files() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        f.write("dir/x.txt", "x\n");
        f.write("dir/y.txt", "y\n");
        let st = repo_status(f.path()).unwrap();
        let paths: Vec<_> = st.files.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, ["dir/x.txt", "dir/y.txt"]);
    }

    #[test]
    fn rename_is_detected_when_staged() {
        let f = Fixture::with_files(&[("old.txt", "some content here\nmore\nlines\n")]);
        std::fs::rename(f.path().join("old.txt"), f.path().join("new.txt")).unwrap();
        stage_files(f.path(), &["old.txt".into(), "new.txt".into()]).unwrap();
        let st = repo_status(f.path()).unwrap();
        assert_eq!(st.files.len(), 1);
        assert_eq!(st.files[0].staged, Some(ChangeKind::Renamed));
        assert_eq!(st.files[0].old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn discard_reverts_tracked_and_removes_new_files_with_backup() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        f.write("a.txt", "edited\n");
        f.write("new.txt", "brand new\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let backup = discard_files(f.path(), &["a.txt".into(), "new.txt".into()])
            .unwrap()
            .expect("backup made");
        assert_eq!(f.read("a.txt"), "one\n");
        assert!(!f.exists("new.txt"));
        assert!(repo_status(f.path()).unwrap().files.is_empty());
        assert_eq!(std::fs::read_to_string(backup.join("a.txt")).unwrap(), "edited\n");
        assert_eq!(std::fs::read_to_string(backup.join("new.txt")).unwrap(), "brand new\n");
    }

    #[test]
    fn paths_outside_the_repo_are_refused() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        for bad in ["../x", "/etc/passwd", "a/../../x", ""] {
            assert!(stage_files(f.path(), &[bad.into()]).is_err(), "{bad}");
            assert!(discard_files(f.path(), &[bad.into()]).is_err(), "{bad}");
        }
    }
}
