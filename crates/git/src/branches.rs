//! Creating, switching, renaming and deleting branches.
//!
//! Switching with uncommitted changes either carries them over (fails if
//! they'd be overwritten) or stashes them first; both are offered by the UI
//! like GitHub Desktop's "leave my changes / bring my changes" prompt.

use std::path::Path;

use git2::{build::CheckoutBuilder, BranchType, ErrorCode, Repository, StashFlags, StatusOptions};

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
    let mut stashed = false;
    if mode == SwitchMode::Stash && is_dirty(repo)? {
        let sig = repo
            .signature()
            .map_err(|_| invalid("git doesn't know who you are: set user.name and user.email"))?;
        let from = current_branch(repo).unwrap_or_else(|| "(detached)".into());
        let message = format!("devspace: changes on {from} before switching to {short}");
        repo.stash_save2(&sig, Some(&message), Some(StashFlags::INCLUDE_UNTRACKED))?;
        stashed = true;
    }
    let result = (|| -> Result<(), git2::Error> {
        let target = repo.revparse_single(refname)?;
        let mut co = CheckoutBuilder::new();
        co.safe();
        repo.checkout_tree(&target, Some(&mut co))?;
        repo.set_head(refname)
    })();
    if let Err(e) = result {
        if stashed {
            // Put the user's work back where it was.
            let _ = repo.stash_pop(0, None);
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
    Ok(stashed)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;
    use crate::{repo_status, stash_list};

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
        let list = stash_list(f.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].message.contains(&main));
    }

    #[test]
    fn stash_mode_on_a_clean_tree_does_not_stash() {
        let (f, _) = two_branches();
        let out = switch_branch(f.path(), "feature", SwitchMode::Stash).unwrap();
        assert!(!out.stashed);
        assert!(stash_list(f.path()).unwrap().is_empty());
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
