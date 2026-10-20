//! The per-branch stash ("shelf"): one stash per branch holding any number
//! of files, which can be stashed and restored one file at a time.
//!
//! Git's own stash can't do this (it is one all-or-nothing entry per call,
//! with no per-file restore and no notion of a branch), so the shelf keeps
//! its own record in the repo: the commit at `refs/devspace/stash/<branch>`
//! has two subtrees, `base/` (each stashed file as it was in HEAD) and `new/`
//! (as it was in the working tree). A file only in `new/` was added, only in
//! `base/` was deleted. Every change makes a new commit whose parent is the
//! previous one, so earlier states stay reachable through the ref's history.
//!
//! Restoring is a three-way merge of (what the file was, what it is now, what
//! was stashed), so edits made since are kept; if both sides changed the same
//! lines the file is left alone and reported. Contents are stored in
//! repository form (LF endings when `core.autocrlf` is on) and converted back
//! on restore.

use std::collections::BTreeMap;
use std::path::Path;

use git2::{
    build::CheckoutBuilder, Index, IndexEntry, IndexTime, ObjectType, Oid, Repository, Signature,
    TreeWalkMode, TreeWalkResult,
};

use crate::{check_rel_path, diff::blob_diff, FileDiff, GitError};

const NAMESPACE: &str = "refs/devspace/stash/";
/// Key used while HEAD is detached.
const DETACHED: &str = "_detached";
const BLOB_MODE: u32 = 0o100644;
const TREE_MODE: i32 = 0o040000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfFile {
    pub path: String,
    pub kind: ShelfKind,
}

/// The stash of the checked-out branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub branch: String,
    /// Sorted by path.
    pub files: Vec<ShelfFile>,
    /// Unix time of the last change.
    pub updated: i64,
}

/// What a stash or restore did, file by file. A file that couldn't be handled
/// is listed with the reason and left exactly where it was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShelfOutcome {
    pub done: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// One file's stashed state: `None` means the file doesn't exist on that side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    base: Option<Oid>,
    new: Option<Oid>,
}

type Entries = BTreeMap<String, Entry>;

fn invalid(msg: impl Into<String>) -> GitError {
    GitError::Invalid(msg.into())
}

/// The key of the checked-out branch (`None` HEAD means detached).
fn branch_key(repo: &Repository) -> String {
    repo.find_reference("HEAD")
        .ok()
        .and_then(|h| h.symbolic_target().map(str::to_owned))
        .map(|t| t.strip_prefix("refs/heads/").unwrap_or(&t).to_owned())
        .unwrap_or_else(|| DETACHED.to_owned())
}

fn ref_name(key: &str) -> String {
    format!("{NAMESPACE}{key}")
}

// ---- line endings -------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Eol {
    /// Files are stored as they are on disk.
    Keep,
    /// `core.autocrlf=input`: LF in the repo, whatever is on disk.
    ToRepo,
    /// `core.autocrlf=true`: LF in the repo, CRLF on disk.
    Crlf,
}

fn eol_mode(repo: &Repository) -> Eol {
    let value = repo
        .config()
        .ok()
        .and_then(|c| c.get_string("core.autocrlf").ok())
        .unwrap_or_default()
        .to_lowercase();
    match value.as_str() {
        "true" => Eol::Crlf,
        "input" => Eol::ToRepo,
        _ => Eol::Keep,
    }
}

fn is_text(bytes: &[u8]) -> bool {
    !bytes.iter().take(8000).any(|b| *b == 0)
}

fn replace_all(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn to_repo_form(bytes: Vec<u8>, eol: Eol) -> Vec<u8> {
    if eol == Eol::Keep || !is_text(&bytes) {
        return bytes;
    }
    replace_all(&bytes, b"\r\n", b"\n")
}

fn to_worktree_form(bytes: Vec<u8>, eol: Eol) -> Vec<u8> {
    if eol != Eol::Crlf || !is_text(&bytes) {
        return bytes;
    }
    let lf = replace_all(&bytes, b"\r\n", b"\n");
    replace_all(&lf, b"\n", b"\r\n")
}

// ---- reading and writing the record ----------------------------------

fn read_side(repo: &Repository, tree: &git2::Tree<'_>, name: &str) -> BTreeMap<String, Oid> {
    let mut out = BTreeMap::new();
    let Some(sub) = tree
        .get_name(name)
        .and_then(|e| e.to_object(repo).ok())
        .and_then(|o| o.into_tree().ok())
    else {
        return out;
    };
    let _ = sub.walk(TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() == Some(ObjectType::Blob) {
            out.insert(format!("{root}{}", entry.name().unwrap_or("")), entry.id());
        }
        TreeWalkResult::Ok
    });
    out
}

/// The branch's record and the commit it came from; `None` when the branch
/// has nothing stashed.
fn load<'r>(
    repo: &'r Repository,
    refname: &str,
) -> Result<Option<(Entries, git2::Commit<'r>)>, GitError> {
    let Ok(reference) = repo.find_reference(refname) else {
        return Ok(None);
    };
    let commit = reference.peel_to_commit()?;
    let tree = commit.tree()?;
    let base = read_side(repo, &tree, "base");
    let new = read_side(repo, &tree, "new");
    let mut entries = Entries::new();
    for path in base.keys().chain(new.keys()) {
        entries.entry(path.clone()).or_insert(Entry {
            base: base.get(path).copied(),
            new: new.get(path).copied(),
        });
    }
    Ok(Some((entries, commit)))
}

fn side_tree(
    repo: &Repository,
    entries: &Entries,
    pick: impl Fn(&Entry) -> Option<Oid>,
) -> Result<Oid, GitError> {
    let mut index = Index::new()?;
    for (path, entry) in entries {
        if let Some(id) = pick(entry) {
            index.add(&IndexEntry {
                ctime: IndexTime::new(0, 0),
                mtime: IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: BLOB_MODE,
                uid: 0,
                gid: 0,
                file_size: 0,
                id,
                flags: 0,
                flags_extended: 0,
                path: path.clone().into_bytes(),
            })?;
        }
    }
    Ok(index.write_tree_to(repo)?)
}

/// Writes `entries` as the branch's new record (a child of the previous
/// one), or removes the ref when nothing is left.
fn save(
    repo: &Repository,
    refname: &str,
    key: &str,
    entries: &Entries,
    previous: Option<&git2::Commit<'_>>,
) -> Result<(), GitError> {
    if entries.is_empty() {
        if let Ok(mut reference) = repo.find_reference(refname) {
            reference.delete()?;
        }
        return Ok(());
    }
    let mut root = repo.treebuilder(None)?;
    root.insert("base", side_tree(repo, entries, |e| e.base)?, TREE_MODE)?;
    root.insert("new", side_tree(repo, entries, |e| e.new)?, TREE_MODE)?;
    let tree = repo.find_tree(root.write()?)?;
    let sig = repo
        .signature()
        .or_else(|_| Signature::now("devspace", "devspace@localhost"))?;
    let message = format!(
        "devspace stash on {key}: {} file{}",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" }
    );
    let parents: Vec<&git2::Commit<'_>> = previous.into_iter().collect();
    repo.commit(Some(refname), &sig, &sig, &message, &tree, &parents)?;
    Ok(())
}

fn kind_of(entry: &Entry) -> ShelfKind {
    match (entry.base, entry.new) {
        (Some(_), Some(_)) => ShelfKind::Modified,
        (None, _) => ShelfKind::Added,
        (Some(_), None) => ShelfKind::Deleted,
    }
}

/// What the checked-out branch has stashed, if anything.
pub fn shelf_list(path: &Path) -> Result<Option<Shelf>, GitError> {
    let repo = Repository::open(path)?;
    let key = branch_key(&repo);
    let Some((entries, commit)) = load(&repo, &ref_name(&key))? else {
        return Ok(None);
    };
    Ok(Some(Shelf {
        branch: key,
        files: entries
            .iter()
            .map(|(path, entry)| ShelfFile {
                path: path.clone(),
                kind: kind_of(entry),
            })
            .collect(),
        updated: commit.time().seconds(),
    }))
}

/// How a stashed file differs from the version it was based on.
pub fn shelf_file_diff(path: &Path, file: &str) -> Result<FileDiff, GitError> {
    check_rel_path(file)?;
    let repo = Repository::open(path)?;
    let key = branch_key(&repo);
    let entry = load(&repo, &ref_name(&key))?
        .and_then(|(entries, _)| entries.get(file).copied())
        .ok_or_else(|| invalid(format!("{file} is not in this branch's stash")))?;
    blob_diff(&repo, entry.base, entry.new, file)
}

// ---- stashing -------------------------------------------------------------

/// What stashing one file needs, once it is known to be possible.
struct Pending {
    path: String,
    /// The file exists in HEAD, so it goes back to that version; otherwise it
    /// is new and is deleted from the working tree.
    in_head: bool,
}

/// Moves the working-tree state of each file into the branch's stash and puts
/// the working tree (and the index) back to HEAD for those files. New files
/// are deleted from disk, deleted files come back; both return on restore.
///
/// Files already in this branch's stash are skipped (restore or discard
/// them first), as are files with no changes, symlinks and submodules. The
/// stash record is written before the working tree is touched.
pub fn shelf_stash(path: &Path, files: &[String]) -> Result<ShelfOutcome, GitError> {
    for file in files {
        check_rel_path(file)?;
    }
    let repo = Repository::open(path)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| invalid("this repository has no working tree"))?
        .to_path_buf();
    let key = branch_key(&repo);
    let refname = ref_name(&key);
    let (mut entries, previous) = match load(&repo, &refname)? {
        Some((entries, commit)) => (entries, Some(commit)),
        None => (Entries::new(), None),
    };
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let eol = eol_mode(&repo);

    let mut outcome = ShelfOutcome::default();
    let mut pending = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for file in files {
        if !seen.insert(file.as_str()) {
            continue;
        }
        let mut skip = |reason: &str| outcome.skipped.push((file.clone(), reason.to_owned()));
        if entries.contains_key(file) {
            skip("already in this branch's stash; restore or discard it first");
            continue;
        }
        let full = workdir.join(file);
        let meta = full.symlink_metadata().ok();
        if meta.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
            skip("symbolic links can't be stashed");
            continue;
        }
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            skip("not a file");
            continue;
        }
        let head_entry = head_tree
            .as_ref()
            .and_then(|t| t.get_path(Path::new(file)).ok());
        if head_entry
            .as_ref()
            .is_some_and(|e| e.kind() != Some(ObjectType::Blob) || e.filemode() == 0o160000)
        {
            skip("not a regular file in HEAD (a folder or submodule)");
            continue;
        }
        let base = head_entry.map(|e| e.id());
        let new = match meta {
            Some(_) => {
                let bytes = std::fs::read(&full)?;
                Some(repo.blob(&to_repo_form(bytes, eol))?)
            }
            None => None,
        };
        if base == new {
            skip("no changes to stash");
            continue;
        }
        entries.insert(file.clone(), Entry { base, new });
        pending.push(Pending {
            path: file.clone(),
            in_head: base.is_some(),
        });
    }
    if pending.is_empty() {
        return Ok(outcome);
    }

    // The record first: if anything below fails, nothing has been lost.
    save(&repo, &refname, &key, &entries, previous.as_ref())?;

    let mut index = repo.index()?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    let mut restore_from_head = false;
    for item in &pending {
        if item.in_head {
            checkout.path(&item.path);
            restore_from_head = true;
        } else {
            let full = workdir.join(&item.path);
            if full.symlink_metadata().is_ok() {
                std::fs::remove_file(&full)?;
            }
            // New and maybe staged: forget the index entry too.
            let _ = index.remove_path(Path::new(&item.path));
        }
    }
    index.write()?;
    if restore_from_head {
        repo.checkout_head(Some(&mut checkout))?;
    }
    outcome.done = pending.into_iter().map(|p| p.path).collect();
    Ok(outcome)
}

// ---- restoring ------------------------------------------------------------

/// Three-way merge of one file's content. `None` when both sides changed the
/// same lines.
fn merge3(
    repo: &Repository,
    base: Oid,
    ours: Oid,
    theirs: Oid,
) -> Result<Option<Vec<u8>>, GitError> {
    let tree_of = |id: Oid| -> Result<git2::Tree<'_>, GitError> {
        let mut builder = repo.treebuilder(None)?;
        builder.insert("f", id, BLOB_MODE as i32)?;
        Ok(repo.find_tree(builder.write()?)?)
    };
    let merged = repo.merge_trees(&tree_of(base)?, &tree_of(ours)?, &tree_of(theirs)?, None)?;
    if merged.has_conflicts() {
        return Ok(None);
    }
    let Some(entry) = merged.get_path(Path::new("f"), 0) else {
        return Ok(None);
    };
    Ok(Some(repo.find_blob(entry.id)?.content().to_vec()))
}

enum Restore {
    Write(Vec<u8>),
    Delete,
    Nothing,
}

/// Decides what restoring one file does, given what is on disk now.
fn plan_restore(
    repo: &Repository,
    entry: &Entry,
    current: Option<Oid>,
) -> Result<Result<Restore, &'static str>, GitError> {
    let content =
        |id: Oid| -> Result<Vec<u8>, GitError> { Ok(repo.find_blob(id)?.content().to_vec()) };
    Ok(match (entry.base, entry.new) {
        // The stash deleted the file.
        (_, None) => match current {
            None => Ok(Restore::Nothing),
            Some(now) if Some(now) == entry.base => Ok(Restore::Delete),
            Some(_) => Err("changed since it was stashed, so it can't be deleted again"),
        },
        // The stash added the file.
        (None, Some(new)) => match current {
            None => Ok(Restore::Write(content(new)?)),
            Some(now) if now == new => Ok(Restore::Nothing),
            Some(_) => Err("a different file with this name exists now"),
        },
        // The stash modified the file.
        (Some(base), Some(new)) => match current {
            None => Err("was deleted since it was stashed"),
            Some(now) if now == base => Ok(Restore::Write(content(new)?)),
            Some(now) if now == new => Ok(Restore::Nothing),
            Some(now) => match merge3(repo, base, now, new)? {
                Some(merged) => Ok(Restore::Write(merged)),
                None => Err("changed in the same place since it was stashed"),
            },
        },
    })
}

/// Puts stashed files back into the working tree and removes them from the
/// stash. Edits made since are kept (three-way merge); a file that would
/// conflict is left in the stash and reported. Restored files are not staged.
pub fn shelf_restore(path: &Path, files: &[String]) -> Result<ShelfOutcome, GitError> {
    for file in files {
        check_rel_path(file)?;
    }
    let repo = Repository::open(path)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| invalid("this repository has no working tree"))?
        .to_path_buf();
    let key = branch_key(&repo);
    let refname = ref_name(&key);
    let Some((mut entries, previous)) = load(&repo, &refname)? else {
        return Err(invalid("nothing is stashed on this branch"));
    };
    let eol = eol_mode(&repo);

    let mut outcome = ShelfOutcome::default();
    let mut seen = std::collections::HashSet::new();
    for file in files {
        if !seen.insert(file.as_str()) {
            continue;
        }
        let Some(entry) = entries.get(file).copied() else {
            outcome
                .skipped
                .push((file.clone(), "isn't in this branch's stash".into()));
            continue;
        };
        let full = workdir.join(file);
        if full
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink() || m.is_dir())
        {
            outcome
                .skipped
                .push((file.clone(), "not a regular file".into()));
            continue;
        }
        let current = match full.symlink_metadata() {
            Ok(_) => Some(repo.blob(&to_repo_form(std::fs::read(&full)?, eol))?),
            Err(_) => None,
        };
        match plan_restore(&repo, &entry, current)? {
            Ok(Restore::Write(bytes)) => {
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full, to_worktree_form(bytes, eol))?;
            }
            Ok(Restore::Delete) => std::fs::remove_file(&full)?,
            Ok(Restore::Nothing) => {}
            Err(reason) => {
                outcome.skipped.push((file.clone(), reason.to_owned()));
                continue;
            }
        }
        entries.remove(file);
        outcome.done.push(file.clone());
    }
    if !outcome.done.is_empty() {
        save(&repo, &refname, &key, &entries, Some(&previous))?;
    }
    Ok(outcome)
}

/// Drops files from the stash without restoring them. The previous state
/// stays reachable from the ref's history until git collects it.
pub fn shelf_discard(path: &Path, files: &[String]) -> Result<ShelfOutcome, GitError> {
    let repo = Repository::open(path)?;
    let key = branch_key(&repo);
    let refname = ref_name(&key);
    let Some((mut entries, previous)) = load(&repo, &refname)? else {
        return Err(invalid("nothing is stashed on this branch"));
    };
    let mut outcome = ShelfOutcome::default();
    for file in files {
        if entries.remove(file).is_some() {
            outcome.done.push(file.clone());
        } else {
            outcome
                .skipped
                .push((file.clone(), "isn't in this branch's stash".into()));
        }
    }
    if !outcome.done.is_empty() {
        save(&repo, &refname, &key, &entries, Some(&previous))?;
    }
    Ok(outcome)
}

/// Moves a branch's stash along with a branch rename.
pub(crate) fn shelf_rename(repo: &Repository, old: &str, new: &str) -> Result<(), GitError> {
    let Ok(mut reference) = repo.find_reference(&ref_name(old)) else {
        return Ok(());
    };
    let Some(target) = reference.target() else {
        return Ok(());
    };
    repo.reference(&ref_name(new), target, false, "devspace: branch renamed")?;
    reference.delete()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;
    use crate::{create_branch, repo_status, switch_branch, SwitchMode};

    const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

    fn names(shelf: &Shelf) -> Vec<(&str, ShelfKind)> {
        shelf
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.kind))
            .collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn stashing_a_file_restores_head_and_lists_it() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        f.write("a.txt", "edited\n");
        let out = shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(out.done, ["a.txt"]);
        assert_eq!(f.read("a.txt"), "one\n");
        assert!(repo_status(f.path()).unwrap().files.is_empty());
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        assert_eq!(names(&shelf), [("a.txt", ShelfKind::Modified)]);
        assert_eq!(shelf.branch, f.branch_name().unwrap());
    }

    #[test]
    fn modified_added_and_deleted_files_all_round_trip() {
        let f = Fixture::with_files(&[("m.txt", "m\n"), ("d.txt", "d\n"), ("keep.txt", "k\n")]);
        f.write("m.txt", "m2\n");
        f.write("new.txt", "n\n");
        f.write("dir/deep.txt", "deep\n");
        std::fs::remove_file(f.path().join("d.txt")).unwrap();
        f.write("keep.txt", "k2\n");
        let all = s(&["m.txt", "new.txt", "dir/deep.txt", "d.txt"]);
        let out = shelf_stash(f.path(), &all).unwrap();
        assert_eq!(out.done.len(), 4, "{:?}", out.skipped);
        // Everything stashed is back at HEAD; the unlisted file is untouched.
        assert_eq!(f.read("m.txt"), "m\n");
        assert!(!f.exists("new.txt") && !f.exists("dir/deep.txt"));
        assert_eq!(f.read("d.txt"), "d\n");
        assert_eq!(f.read("keep.txt"), "k2\n");
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        assert_eq!(
            names(&shelf),
            [
                ("d.txt", ShelfKind::Deleted),
                ("dir/deep.txt", ShelfKind::Added),
                ("m.txt", ShelfKind::Modified),
                ("new.txt", ShelfKind::Added),
            ]
        );

        let back = shelf_restore(f.path(), &all).unwrap();
        assert_eq!(back.done.len(), 4, "{:?}", back.skipped);
        assert_eq!(f.read("m.txt"), "m2\n");
        assert_eq!(f.read("new.txt"), "n\n");
        assert_eq!(f.read("dir/deep.txt"), "deep\n");
        assert!(!f.exists("d.txt"));
        assert!(
            shelf_list(f.path()).unwrap().is_none(),
            "ref removed when empty"
        );
        assert!(f
            .repo
            .find_reference(&ref_name(&f.branch_name().unwrap()))
            .is_err());
    }

    #[test]
    fn staged_changes_are_stashed_and_unstaged() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        f.write("a.txt", "staged\n");
        f.write("new.txt", "n\n");
        crate::stage_files(f.path(), &s(&["a.txt", "new.txt"])).unwrap();
        shelf_stash(f.path(), &s(&["a.txt", "new.txt"])).unwrap();
        assert!(repo_status(f.path()).unwrap().files.is_empty());
    }

    #[test]
    fn partial_stash_and_partial_restore() {
        let f = Fixture::with_files(&[("a.txt", "1\n"), ("b.txt", "1\n"), ("c.txt", "1\n")]);
        for n in ["a.txt", "b.txt", "c.txt"] {
            f.write(n, "2\n");
        }
        shelf_stash(f.path(), &s(&["a.txt", "b.txt"])).unwrap();
        assert_eq!(f.read("c.txt"), "2\n");
        // One restored, one stays.
        shelf_restore(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(f.read("a.txt"), "2\n");
        assert_eq!(f.read("b.txt"), "1\n");
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        assert_eq!(names(&shelf), [("b.txt", ShelfKind::Modified)]);
    }

    #[test]
    fn there_is_one_stash_per_branch_and_stashing_adds_to_it() {
        let f = Fixture::with_files(&[("a.txt", "1\n"), ("b.txt", "1\n")]);
        f.write("a.txt", "2\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        f.write("b.txt", "2\n");
        shelf_stash(f.path(), &s(&["b.txt"])).unwrap();
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        assert_eq!(shelf.files.len(), 2);
        // The history keeps the earlier state, but there is one ref.
        let refs: Vec<_> = f
            .repo
            .references_glob(&format!("{NAMESPACE}*"))
            .unwrap()
            .collect();
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn a_file_already_stashed_is_not_stashed_over() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "first\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        f.write("a.txt", "second\n");
        let out = shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        assert!(out.done.is_empty());
        assert!(out.skipped[0].1.contains("already in this branch's stash"));
        assert_eq!(f.read("a.txt"), "second\n", "left alone");
        // Restoring over a different edit must not silently overwrite it.
        let out = shelf_restore(f.path(), &s(&["a.txt"])).unwrap();
        assert!(out.done.is_empty(), "{out:?}");
        assert!(out.skipped[0].1.contains("same place"), "{out:?}");
    }

    #[test]
    fn unchanged_files_are_skipped() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        let out = shelf_stash(f.path(), &s(&["a.txt", "nope.txt"])).unwrap();
        assert!(out.done.is_empty());
        assert_eq!(out.skipped.len(), 2);
        assert!(shelf_list(f.path()).unwrap().is_none());
    }

    #[test]
    fn each_branch_has_its_own_stash() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        let main = f.branch_name().unwrap();
        f.write("a.txt", "on main\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();

        create_branch(f.path(), "feature", None, SwitchMode::Bring).unwrap();
        assert!(
            shelf_list(f.path()).unwrap().is_none(),
            "main's stash is not here"
        );
        f.write("a.txt", "on feature\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(shelf_list(f.path()).unwrap().unwrap().branch, "feature");

        switch_branch(f.path(), &main, SwitchMode::Bring).unwrap();
        assert_eq!(shelf_list(f.path()).unwrap().unwrap().branch, main);
        shelf_restore(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(f.read("a.txt"), "on main\n");
    }

    #[test]
    fn restore_keeps_edits_made_in_the_meantime() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        // A different part of the file is edited while it is stashed.
        f.write("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\nNINE\n10\n");
        let out = shelf_restore(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(out.done, ["a.txt"], "{:?}", out.skipped);
        assert_eq!(f.read("a.txt"), "1\nTWO\n3\n4\n5\n6\n7\n8\nNINE\n10\n");
    }

    #[test]
    fn restore_after_the_branch_moved_on() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        // A commit changes another part of the same file.
        f.write("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\nTEN\n");
        f.stage_all();
        f.commit("elsewhere");
        let out = shelf_restore(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(out.done, ["a.txt"], "{:?}", out.skipped);
        assert_eq!(f.read("a.txt"), "1\nTWO\n3\n4\n5\n6\n7\n8\n9\nTEN\n");
    }

    #[test]
    fn a_conflicting_restore_changes_nothing_and_keeps_the_stash() {
        let f = Fixture::with_files(&[("a.txt", "1\n2\n3\n"), ("b.txt", "b\n")]);
        f.write("a.txt", "1\nmine\n3\n");
        f.write("b.txt", "b2\n");
        shelf_stash(f.path(), &s(&["a.txt", "b.txt"])).unwrap();
        f.write("a.txt", "1\ntheirs\n3\n");
        let out = shelf_restore(f.path(), &s(&["a.txt", "b.txt"])).unwrap();
        assert_eq!(out.done, ["b.txt"]);
        assert_eq!(out.skipped.len(), 1);
        assert!(out.skipped[0].1.contains("same place"));
        assert_eq!(f.read("a.txt"), "1\ntheirs\n3\n", "not touched");
        assert_eq!(f.read("b.txt"), "b2\n");
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        assert_eq!(names(&shelf), [("a.txt", ShelfKind::Modified)]);
    }

    #[test]
    fn restoring_an_added_file_over_a_different_one_is_refused() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("new.txt", "stashed\n");
        shelf_stash(f.path(), &s(&["new.txt"])).unwrap();
        f.write("new.txt", "something else\n");
        let out = shelf_restore(f.path(), &s(&["new.txt"])).unwrap();
        assert!(out.done.is_empty());
        assert_eq!(f.read("new.txt"), "something else\n");
    }

    #[test]
    fn discard_drops_files_and_the_last_one_removes_the_ref() {
        let f = Fixture::with_files(&[("a.txt", "1\n"), ("b.txt", "1\n")]);
        f.write("a.txt", "2\n");
        f.write("b.txt", "2\n");
        shelf_stash(f.path(), &s(&["a.txt", "b.txt"])).unwrap();
        shelf_discard(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(shelf_list(f.path()).unwrap().unwrap().files.len(), 1);
        shelf_discard(f.path(), &s(&["b.txt"])).unwrap();
        assert!(shelf_list(f.path()).unwrap().is_none());
        // Discarded is gone: the working tree stays at HEAD.
        assert_eq!(f.read("a.txt"), "1\n");
    }

    #[test]
    fn the_diff_of_a_stashed_file_is_against_its_base() {
        let f = Fixture::with_files(&[("a.txt", "a\nb\nc\n")]);
        f.write("a.txt", "a\nB\nc\n");
        f.write("new.txt", "x\ny\n");
        shelf_stash(f.path(), &s(&["a.txt", "new.txt"])).unwrap();
        let d = shelf_file_diff(f.path(), "a.txt").unwrap();
        let texts: Vec<_> = d.hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind != crate::LineKind::Context)
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                (crate::LineKind::Removed, "b"),
                (crate::LineKind::Added, "B")
            ]
        );
        let added = shelf_file_diff(f.path(), "new.txt").unwrap();
        assert!(added.old_missing);
        assert_eq!(added.all_changed().len(), 2);
        assert!(shelf_file_diff(f.path(), "missing.txt").is_err());
    }

    #[test]
    fn crlf_files_round_trip_byte_for_byte_with_autocrlf() {
        let f = Fixture::empty();
        f.repo
            .config()
            .unwrap()
            .set_str("core.autocrlf", "true")
            .unwrap();
        f.write("a.txt", "a\r\nb\r\nc\r\nd\r\ne\r\n");
        f.stage_all();
        f.commit("crlf");
        f.write("a.txt", "a\r\nB\r\nc\r\nd\r\ne\r\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(
            std::fs::read(f.path().join("a.txt")).unwrap(),
            b"a\r\nb\r\nc\r\nd\r\ne\r\n"
        );
        // An unrelated edit while stashed is merged, not flagged as a rewrite.
        f.write("a.txt", "a\r\nb\r\nc\r\nd\r\nE\r\n");
        let out = shelf_restore(f.path(), &s(&["a.txt"])).unwrap();
        assert_eq!(out.done, ["a.txt"], "{:?}", out.skipped);
        assert_eq!(
            std::fs::read(f.path().join("a.txt")).unwrap(),
            b"a\r\nB\r\nc\r\nd\r\nE\r\n"
        );
    }

    #[test]
    fn binary_files_stash_and_restore() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        std::fs::write(f.path().join("img.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        shelf_stash(f.path(), &s(&["img.bin"])).unwrap();
        assert!(!f.exists("img.bin"));
        assert!(shelf_file_diff(f.path(), "img.bin").unwrap().binary);
        shelf_restore(f.path(), &s(&["img.bin"])).unwrap();
        assert_eq!(
            std::fs::read(f.path().join("img.bin")).unwrap(),
            [0u8, 159, 146, 150, 0, 1]
        );
    }

    #[test]
    fn renaming_a_branch_keeps_its_stash() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        create_branch(f.path(), "old-name", None, SwitchMode::Bring).unwrap();
        f.write("a.txt", "2\n");
        shelf_stash(f.path(), &s(&["a.txt"])).unwrap();
        crate::rename_branch(f.path(), "old-name", "new-name").unwrap();
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        assert_eq!(shelf.branch, "new-name");
        assert_eq!(shelf.files.len(), 1);
    }

    #[test]
    fn paths_outside_the_repo_are_refused() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        for bad in ["../x", "/etc/passwd", ""] {
            assert!(shelf_stash(f.path(), &s(&[bad])).is_err(), "{bad:?}");
            assert!(shelf_restore(f.path(), &s(&[bad])).is_err(), "{bad:?}");
        }
    }
}
