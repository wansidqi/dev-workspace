//! Creating, switching, renaming and deleting branches.
//!
//! Switching with uncommitted changes either carries them over (fails if
//! they'd be overwritten) or stashes them first; both are offered by the UI
//! like GitHub Desktop's "leave my changes / bring my changes" prompt.

use std::path::Path;

use git2::{build::CheckoutBuilder, BranchType, ErrorCode, Repository, StatusOptions};

use crate::{is_safe_ref_name, GitError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchMode {
    /// Carry uncommitted changes onto the other branch. Fails, changing
    /// nothing, if the switch would overwrite any of them.
    Bring,
    /// Stash uncommitted changes (untracked files too) before switching.
    Stash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchOutcome {
    pub branch: String,
    /// Changes were put in the stash.
    pub stashed: bool,
    /// The branch only existed as `origin/<name>` and a tracking branch was
    /// made for it.
    pub created_from_remote: bool,
}

fn invalid(msg: impl Into<String>) -> GitError {
    GitError::Invalid(msg.into())
}

fn check_new_name(name: &str) -> Result<(), GitError> {
    if is_safe_ref_name(name) && git2::Branch::name_is_valid(name).unwrap_or(false) {
        Ok(())
    } else {
        Err(invalid(format!("`{name}` isn't a valid branch name")))
    }
}

fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.find_reference("HEAD").ok()?;
    head.symbolic_target()
        .map(|t| t.strip_prefix("refs/heads/").unwrap_or(t).to_owned())
}

fn is_dirty(repo: &Repository) -> Result<bool, GitError> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).include_ignored(false);
    Ok(!repo.statuses(Some(&mut opts))?.is_empty())
}

fn refuse_mid_operation(repo: &Repository) -> Result<(), GitError> {
    if repo.state() == git2::RepositoryState::Clean {
        Ok(())
    } else {
        Err(invalid(
            "finish or abort the merge, rebase or cherry-pick in progress first",
        ))
    }
}

/// Moves HEAD to `refname`, updating the working tree.
fn checkout_ref(
    repo: &mut Repository,
    refname: &str,
    short: &str,
    mode: SwitchMode,
) -> Result<bool, GitError> {
    // Files put in this branch's stash before switching.
    let mut stashed: Vec<String> = Vec::new();
    let workdir = repo.workdir().map(Path::to_path_buf);
    if mode == SwitchMode::Stash && is_dirty(repo)? {
        let workdir = workdir
            .clone()
            .ok_or_else(|| invalid("this repository has no working tree"))?;
        let status = crate::repo_status(&workdir)?;
        let files: Vec<String> = status
            .files
            .iter()
            .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
            .collect();
        let outcome = crate::shelf_stash(&workdir, &files)?;
        stashed = outcome.done;
        // A file that is already in the stash (or can't be stashed) would be
        // overwritten by the switch or carried along; stop before anything moves.
        if let Some((file, reason)) = outcome
            .skipped
            .iter()
            .find(|(_, reason)| !reason.starts_with("no changes"))
        {
            let message = format!("couldn't stash {file}: {reason}");
            let _ = crate::shelf_restore(&workdir, &stashed);
            return Err(invalid(message));
        }
    }
    let result = (|| -> Result<(), git2::Error> {
        let target = repo.revparse_single(refname)?;
        let mut co = CheckoutBuilder::new();
        co.safe();
        repo.checkout_tree(&target, Some(&mut co))?;
        repo.set_head(refname)
    })();
    if let Err(e) = result {
        if let (false, Some(workdir)) = (stashed.is_empty(), &workdir) {
            // Put the user's work back where it was.
            let _ = crate::shelf_restore(workdir, &stashed);
        }
        return Err(if e.code() == ErrorCode::Conflict {
            invalid(format!(
                "switching to `{short}` would overwrite your uncommitted changes; \
                 commit them, or switch with \"stash my changes\""
            ))
        } else {
            e.into()
        });
    }
    Ok(!stashed.is_empty())
}

/// Checks out a local branch, or makes a tracking branch from `origin/<name>`
/// when only that exists.
pub fn switch_branch(path: &Path, name: &str, mode: SwitchMode) -> Result<SwitchOutcome, GitError> {
    if !is_safe_ref_name(name) {
        return Err(invalid(format!("`{name}` isn't a valid branch name")));
    }
    let mut repo = Repository::open(path)?;
    refuse_mid_operation(&repo)?;
    let mut outcome = SwitchOutcome {
        branch: name.to_owned(),
        stashed: false,
        created_from_remote: false,
    };
    if current_branch(&repo).as_deref() == Some(name) {
        return Ok(outcome);
    }

    let refname = format!("refs/heads/{name}");
    if repo.find_branch(name, BranchType::Local).is_err() {
        let remote = format!("origin/{name}");
        let commit = repo
            .find_reference(&format!("refs/remotes/{remote}"))
            .ok()
            .and_then(|r| r.peel_to_commit().ok())
            .ok_or_else(|| invalid(format!("there is no branch `{name}`")))?;
        let mut branch = repo.branch(name, &commit, false)?;
        branch.set_upstream(Some(&remote))?;
        outcome.created_from_remote = true;
    }
    match checkout_ref(&mut repo, &refname, name, mode) {
        Ok(stashed) => outcome.stashed = stashed,
        Err(e) => {
            if outcome.created_from_remote {
                // Don't leave a half-made branch behind.
                if let Ok(mut b) = repo.find_branch(name, BranchType::Local) {
                    let _ = b.delete();
                }
            }
            return Err(e);
        }
    }
    Ok(outcome)
}

/// Creates a branch and switches to it. `from` is the branch (or any
/// revision) to start at; `None` starts at the current commit, which keeps
/// uncommitted changes in place without stashing.
pub fn create_branch(
    path: &Path,
    name: &str,
    from: Option<&str>,
    mode: SwitchMode,
) -> Result<SwitchOutcome, GitError> {
    check_new_name(name)?;
    let mut repo = Repository::open(path)?;
    refuse_mid_operation(&repo)?;
    if repo.find_branch(name, BranchType::Local).is_ok() {
        return Err(invalid(format!("a branch named `{name}` already exists")));
    }
    let head_id = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id());
    let start_id = match from {
        None => head_id.ok_or_else(|| invalid("make a first commit before creating branches"))?,
        Some(from) => {
            if !is_safe_ref_name(from) {
                return Err(invalid(format!("`{from}` isn't a valid starting point")));
            }
            repo.revparse_single(from)
                .or_else(|_| repo.revparse_single(&format!("origin/{from}")))
                .and_then(|o| o.peel_to_commit())
                .map(|c| c.id())
                .map_err(|_| invalid(format!("there is no branch `{from}`")))?
        }
    };
    {
        let start = repo.find_commit(start_id)?;
        repo.branch(name, &start, false)?;
    }
    let refname = format!("refs/heads/{name}");
    let result = if head_id == Some(start_id) {
        // Same commit: only HEAD moves, the working tree stays as it is.
        repo.set_head(&refname)
            .map(|_| false)
            .map_err(GitError::from)
    } else {
        checkout_ref(&mut repo, &refname, name, mode)
    };
    match result {
        Ok(stashed) => Ok(SwitchOutcome {
            branch: name.to_owned(),
            stashed,
            created_from_remote: false,
        }),
        Err(e) => {
            if let Ok(mut b) = repo.find_branch(name, BranchType::Local) {
                let _ = b.delete();
            }
            Err(e)
        }
    }
}

pub fn rename_branch(path: &Path, old: &str, new: &str) -> Result<(), GitError> {
    check_new_name(new)?;
    if !is_safe_ref_name(old) {
        return Err(invalid(format!("`{old}` isn't a valid branch name")));
    }
    let repo = Repository::open(path)?;
    if repo.find_branch(new, BranchType::Local).is_ok() {
        return Err(invalid(format!("a branch named `{new}` already exists")));
    }
    let mut branch = repo
        .find_branch(old, BranchType::Local)
        .map_err(|_| invalid(format!("there is no local branch `{old}`")))?;
    branch.rename(new, false)?;
    crate::shelf::shelf_rename(&repo, old, new)?;
    Ok(())
}

/// Deletes a local branch. The checked-out branch can't be deleted. Branches
/// with commits that aren't in the current branch need `force`; without it
/// they fail with [`GitError::NotMerged`] so the UI can ask first.
pub fn delete_branch(path: &Path, name: &str, force: bool) -> Result<(), GitError> {
    if !is_safe_ref_name(name) {
        return Err(invalid(format!("`{name}` isn't a valid branch name")));
    }
    let repo = Repository::open(path)?;
    if current_branch(&repo).as_deref() == Some(name) {
        return Err(invalid("switch to another branch before deleting this one"));
    }
    let mut branch = repo
        .find_branch(name, BranchType::Local)
        .map_err(|_| invalid(format!("there is no local branch `{name}`")))?;
    if !force {
        let tip = branch.get().peel_to_commit()?.id();
        let merged = match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(head) => head.id() == tip || repo.graph_descendant_of(head.id(), tip)?,
            None => false,
        };
        if !merged {
            return Err(GitError::NotMerged(name.to_owned()));
        }
    }
    branch.delete()?;
    Ok(())
}

/// A branch you were on recently, and when you last left or entered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentBranch {
    pub name: String,
    /// Unix time of that checkout.
    pub time: i64,
}

/// The local branches you checked out most recently, newest first, read from
/// HEAD's reflog (the "checkout: moving from A to B" lines git writes). Both
/// ends of a move count. Only branches that still exist are returned, so
/// deleted ones vanish; the reflog is local, so a fresh clone has none.
pub fn recent_branches(path: &Path, limit: usize) -> Result<Vec<RecentBranch>, GitError> {
    let repo = Repository::open(path)?;
    let Ok(reflog) = repo.reflog("HEAD") else {
        return Ok(Vec::new());
    };
    let mut out: Vec<RecentBranch> = Vec::new();
    for entry in reflog.iter() {
        let Some(message) = entry.message() else {
            continue;
        };
        let Some(rest) = message.strip_prefix("checkout: moving from ") else {
            continue;
        };
        let Some((from, to)) = rest.rsplit_once(" to ") else {
            continue;
        };
        let time = entry.committer().when().seconds();
        for name in [to, from] {
            if out.len() >= limit {
                break;
            }
            let name = name.trim();
            if out.iter().any(|r| r.name == name)
                || repo.find_branch(name, BranchType::Local).is_err()
            {
                continue;
            }
            out.push(RecentBranch {
                name: name.to_owned(),
                time,
            });
        }
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;
    use crate::{repo_status, shelf_list};

    /// Repo on its default branch with `a.txt` and a second branch `feature`
    /// that changes `a.txt` and adds `f.txt`.
    fn two_branches() -> (Fixture, String) {
        let f = Fixture::with_files(&[("a.txt", "base\n"), ("keep.txt", "keep\n")]);
        let main = f.branch_name().unwrap();
        create_branch(f.path(), "feature", None, SwitchMode::Bring).unwrap();
        f.write("a.txt", "feature\n");
        f.write("f.txt", "f\n");
        f.stage_all();
        f.commit("feature work");
        switch_branch(f.path(), &main, SwitchMode::Bring).unwrap();
        (f, main)
    }

    #[test]
    fn create_branch_keeps_uncommitted_changes() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        let out = create_branch(f.path(), "task-7", None, SwitchMode::Bring).unwrap();
        assert!(!out.stashed);
        assert_eq!(f.branch_name().as_deref(), Some("task-7"));
        assert_eq!(repo_status(f.path()).unwrap().files.len(), 1);
    }

    #[test]
    fn invalid_and_duplicate_names_are_refused() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        for bad in ["", "-x", "has space", "a..b", "x.lock", "bad~name"] {
            assert!(
                create_branch(f.path(), bad, None, SwitchMode::Bring).is_err(),
                "{bad:?}"
            );
        }
        create_branch(f.path(), "dup", None, SwitchMode::Bring).unwrap();
        assert!(create_branch(f.path(), "dup", None, SwitchMode::Bring).is_err());
    }

    #[test]
    fn switch_carries_compatible_changes() {
        let (f, _main) = two_branches();
        f.write("keep.txt", "edited\n");
        let out = switch_branch(f.path(), "feature", SwitchMode::Bring).unwrap();
        assert!(!out.stashed);
        assert_eq!(f.branch_name().as_deref(), Some("feature"));
        assert_eq!(f.read("keep.txt"), "edited\n");
        assert_eq!(f.read("a.txt"), "feature\n");
    }

    #[test]
    fn switch_that_would_overwrite_changes_fails_and_changes_nothing() {
        let (f, main) = two_branches();
        f.write("a.txt", "my local edit\n");
        let err = switch_branch(f.path(), "feature", SwitchMode::Bring).unwrap_err();
        assert!(err.to_string().contains("overwrite"), "{err}");
        assert_eq!(f.branch_name().as_deref(), Some(main.as_str()));
        assert_eq!(f.read("a.txt"), "my local edit\n");
    }

    #[test]
    fn switch_with_stash_cleans_the_tree_and_keeps_the_work() {
        let (f, main) = two_branches();
        f.write("a.txt", "my local edit\n");
        f.write("scratch.txt", "untracked\n");
        let out = switch_branch(f.path(), "feature", SwitchMode::Stash).unwrap();
        assert!(out.stashed);
        assert_eq!(f.read("a.txt"), "feature\n");
        assert!(!f.exists("scratch.txt"));
        // The work is in the stash of the branch it was made on.
        let shelf = shelf_list(f.path()).unwrap();
        assert!(shelf.is_none(), "not on the new branch");
        switch_branch(f.path(), &main, SwitchMode::Bring).unwrap();
        let shelf = shelf_list(f.path()).unwrap().unwrap();
        let files: Vec<_> = shelf.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(files, ["a.txt", "scratch.txt"]);
    }

    #[test]
    fn stash_mode_on_a_clean_tree_does_not_stash() {
        let (f, _) = two_branches();
        let out = switch_branch(f.path(), "feature", SwitchMode::Stash).unwrap();
        assert!(!out.stashed);
        assert!(shelf_list(f.path()).unwrap().is_none());
    }

    #[test]
    fn switching_to_a_remote_only_branch_makes_a_tracking_branch() {
        let (f, main) = two_branches();
        // Pretend `feature` only exists on origin.
        let tip = f
            .repo
            .find_branch("feature", BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap();
        f.repo
            .reference("refs/remotes/origin/remote-only", tip.id(), true, "test")
            .unwrap();
        f.repo
            .remote("origin", "https://example.invalid/x.git")
            .unwrap();
        let out = switch_branch(f.path(), "remote-only", SwitchMode::Bring).unwrap();
        assert!(out.created_from_remote);
        assert_eq!(f.branch_name().as_deref(), Some("remote-only"));
        let b = f
            .repo
            .find_branch("remote-only", BranchType::Local)
            .unwrap();
        assert!(b.upstream().is_ok());
        let _ = main;
    }

    #[test]
    fn unknown_branch_is_an_error() {
        let (f, _) = two_branches();
        assert!(switch_branch(f.path(), "nope", SwitchMode::Bring).is_err());
        assert!(switch_branch(f.path(), "--force", SwitchMode::Bring).is_err());
    }

    #[test]
    fn create_from_another_branch_switches_the_tree() {
        let (f, _) = two_branches();
        create_branch(f.path(), "child", Some("feature"), SwitchMode::Bring).unwrap();
        assert_eq!(f.read("a.txt"), "feature\n");
        assert!(f.exists("f.txt"));
    }

    #[test]
    fn recent_branches_follow_checkouts_newest_first() {
        let f = Fixture::with_files(&[(
            "a.txt", "1
",
        )]);
        let main = f.branch_name().unwrap();
        for name in ["one", "two", "three"] {
            create_branch(f.path(), name, None, SwitchMode::Bring).unwrap();
            switch_branch(f.path(), &main, SwitchMode::Bring).unwrap();
        }
        switch_branch(f.path(), "two", SwitchMode::Bring).unwrap();
        let names = |limit| -> Vec<String> {
            recent_branches(f.path(), limit)
                .unwrap()
                .into_iter()
                .map(|r| r.name)
                .collect()
        };
        // Currently on `two`, which was entered last, coming from main.
        let all = names(10);
        assert_eq!(all[0], "two");
        assert!(all.contains(&main) && all.contains(&"three".to_string()));
        assert_eq!(all.iter().filter(|n| *n == "two").count(), 1, "no repeats");
        assert_eq!(names(2).len(), 2, "limited");
        // A deleted branch is gone from the list.
        switch_branch(f.path(), &main, SwitchMode::Bring).unwrap();
        delete_branch(f.path(), "one", true).unwrap();
        assert!(!names(10).contains(&"one".to_string()));
        // A repo with no history of switching has none.
        assert!(
            recent_branches(Fixture::with_files(&[("x", "1")]).path(), 5)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rename_and_delete() {
        let (f, main) = two_branches();
        rename_branch(f.path(), "feature", "feature-2").unwrap();
        assert!(f.repo.find_branch("feature-2", BranchType::Local).is_ok());
        assert!(rename_branch(f.path(), "feature-2", &main).is_err());

        // Unmerged: needs force.
        let err = delete_branch(f.path(), "feature-2", false).unwrap_err();
        assert!(matches!(err, GitError::NotMerged(_)));
        delete_branch(f.path(), "feature-2", true).unwrap();
        assert!(f.repo.find_branch("feature-2", BranchType::Local).is_err());
    }

    #[test]
    fn merged_branches_delete_without_force_but_not_the_current_one() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        let main = f.branch_name().unwrap();
        create_branch(f.path(), "done", None, SwitchMode::Bring).unwrap();
        switch_branch(f.path(), &main, SwitchMode::Bring).unwrap();
        delete_branch(f.path(), "done", false).unwrap();
        assert!(delete_branch(f.path(), &main, true).is_err());
    }
}
