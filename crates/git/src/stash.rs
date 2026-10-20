//! Stash: save, list, apply, pop, drop. Entries are addressed by their
//! position in the list (0 is the newest), which shifts after a drop or pop,
//! so callers reload the list after every change.

use std::path::Path;

use git2::{ErrorCode, Repository, StashFlags};

use crate::GitError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub time: i64,
}

/// Stashes tracked changes (and untracked files when asked), leaving a clean
/// working tree. `Ok(false)` when there was nothing to stash.
pub fn stash_save(
    path: &Path,
    message: Option<&str>,
    include_untracked: bool,
) -> Result<bool, GitError> {
    let mut repo = Repository::open(path)?;
    let sig = repo.signature().map_err(|_| {
        GitError::Invalid("git doesn't know who you are: set user.name and user.email".into())
    })?;
    let flags = include_untracked.then_some(StashFlags::INCLUDE_UNTRACKED);
    let message = message.map(str::trim).filter(|m| !m.is_empty());
    match repo.stash_save2(&sig, message, flags) {
        Ok(_) => Ok(true),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn stash_list(path: &Path) -> Result<Vec<StashEntry>, GitError> {
    let mut repo = Repository::open(path)?;
    let mut raw = Vec::new();
    repo.stash_foreach(|index, message, oid| {
        raw.push((index, message.to_owned(), *oid));
        true
    })?;
    Ok(raw
        .into_iter()
        .map(|(index, message, oid)| StashEntry {
            index,
            message,
            time: repo
                .find_commit(oid)
                .map(|c| c.time().seconds())
                .unwrap_or(0),
        })
        .collect())
}

fn apply_error(e: git2::Error) -> GitError {
    if e.code() == ErrorCode::Conflict {
        GitError::Invalid(
            "the stash doesn't apply cleanly over your current changes; commit or stash them first"
                .into(),
        )
    } else {
        e.into()
    }
}

/// Re-applies a stash and keeps it in the list.
pub fn stash_apply(path: &Path, index: usize) -> Result<(), GitError> {
    let mut repo = Repository::open(path)?;
    repo.stash_apply(index, None).map_err(apply_error)
}

/// Re-applies a stash and removes it from the list.
pub fn stash_pop(path: &Path, index: usize) -> Result<(), GitError> {
    let mut repo = Repository::open(path)?;
    repo.stash_pop(index, None).map_err(apply_error)
}

pub fn stash_drop(path: &Path, index: usize) -> Result<(), GitError> {
    let mut repo = Repository::open(path)?;
    repo.stash_drop(index)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_status;
    use crate::testutil::Fixture;

    #[test]
    fn save_list_pop_round_trip() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        f.write("new.txt", "n\n");
        assert!(stash_save(f.path(), Some("my work"), true).unwrap());
        assert!(repo_status(f.path()).unwrap().files.is_empty());
        assert!(!f.exists("new.txt"));

        let list = stash_list(f.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].message.contains("my work"));

        stash_pop(f.path(), 0).unwrap();
        assert_eq!(f.read("a.txt"), "2\n");
        assert!(f.exists("new.txt"));
        assert!(stash_list(f.path()).unwrap().is_empty());
    }

    #[test]
    fn untracked_files_stay_unless_asked() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        f.write("new.txt", "n\n");
        assert!(stash_save(f.path(), None, false).unwrap());
        assert!(f.exists("new.txt"));
        assert_eq!(f.read("a.txt"), "1\n");
    }

    #[test]
    fn nothing_to_stash_is_not_an_error() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        assert!(!stash_save(f.path(), None, true).unwrap());
    }

    #[test]
    fn apply_keeps_the_entry_and_drop_removes_it() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        stash_save(f.path(), None, false).unwrap();
        stash_apply(f.path(), 0).unwrap();
        assert_eq!(f.read("a.txt"), "2\n");
        assert_eq!(stash_list(f.path()).unwrap().len(), 1);
        stash_drop(f.path(), 0).unwrap();
        assert!(stash_list(f.path()).unwrap().is_empty());
    }

    #[test]
    fn conflicting_apply_reports_clearly() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "stashed\n");
        stash_save(f.path(), None, false).unwrap();
        f.write("a.txt", "local edit\n");
        let err = stash_apply(f.path(), 0).unwrap_err();
        assert!(matches!(err, GitError::Invalid(_)), "{err}");
        assert_eq!(f.read("a.txt"), "local edit\n");
    }
}
