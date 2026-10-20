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
    /// Branch the stash was made on, read from its message (`WIP on <branch>:`
    /// or `On <branch>:`, which git and GitHub Desktop both write). `None`
    /// when the message doesn't say.
    pub branch: Option<String>,
}

/// The branch named at the start of a stash message.
fn parse_stash_branch(message: &str) -> Option<String> {
    let rest = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))?;
    let (branch, _) = rest.split_once(':')?;
    let branch = branch.trim();
    (!branch.is_empty() && !branch.contains(' ')).then(|| branch.to_owned())
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
            branch: parse_stash_branch(&message),
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
    fn the_branch_is_read_from_the_message() {
        let b = |m: &str| parse_stash_branch(m);
        assert_eq!(
            b("WIP on src-preprod-test: 104a1e7 msg").as_deref(),
            Some("src-preprod-test")
        );
        assert_eq!(
            b("On master: !!GitHub_Desktop<master>").as_deref(),
            Some("master")
        );
        assert_eq!(
            b("On feature/a-b: my note: with colon").as_deref(),
            Some("feature/a-b")
        );
        assert_eq!(b("On (no branch): x"), None);
        assert_eq!(b("custom message"), None);
        assert_eq!(b("On : x"), None);
    }

    #[test]
    fn listed_stashes_know_their_branch() {
        let f = Fixture::with_files(&[(
            "a.txt", "1
",
        )]);
        let branch = f.branch_name().unwrap();
        f.write(
            "a.txt", "2
",
        );
        stash_save(f.path(), Some("note"), false).unwrap();
        f.write(
            "a.txt", "3
",
        );
        stash_save(f.path(), None, false).unwrap();
        let list = stash_list(f.path()).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list
            .iter()
            .all(|s| s.branch.as_deref() == Some(branch.as_str())));
    }

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
