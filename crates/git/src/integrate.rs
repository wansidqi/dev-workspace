//! Merge, conflict resolution, revert, cherry-pick and undo (Phase 3).
//!
//! Merge, revert and cherry-pick go through the git CLI (they can run hooks
//! and leave an operation half done for the user to finish); previews and
//! "undo last commit" use libgit2.

use std::path::Path;

use git2::{Repository, ResetType};

use crate::cli::{is_full_hash, run, run_ok, Kind};
use crate::{check_rel_path, is_safe_ref_name, repo_status, GitError};

fn invalid(msg: impl Into<String>) -> GitError {
    GitError::Invalid(msg.into())
}

/// What a merge would do, read without touching the working tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergePreview {
    /// Commits of the other branch that are not in the current one.
    pub commits: usize,
    /// Files that would conflict. Empty when the merge is clean.
    pub conflicts: Vec<String>,
    /// The other branch is already contained in the current one.
    pub up_to_date: bool,
    /// The merge would only move the branch pointer forward.
    pub fast_forward: bool,
}

/// How a merge ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Merged (or fast-forwarded).
    Merged,
    UpToDate,
    /// Stopped with these files to resolve.
    Conflicts(usize),
}

fn resolve_commit<'r>(repo: &'r Repository, name: &str) -> Result<git2::Commit<'r>, GitError> {
    if !is_safe_ref_name(name) {
        return Err(invalid(format!("`{name}` isn't a usable branch name")));
    }
    let object = repo
        .revparse_single(name)
        .map_err(|_| invalid(format!("there is no branch `{name}`")))?;
    Ok(object.peel_to_commit()?)
}

/// What merging `branch` into the current branch would do (GitHub Desktop's
/// "N commits will be merged / conflicts detected" line).
pub fn merge_preview(path: &Path, branch: &str) -> Result<MergePreview, GitError> {
    let repo = Repository::open(path)?;
    let head = repo
        .head()
        .map_err(|_| invalid("the current branch has no commits yet"))?
        .peel_to_commit()?;
    let theirs = resolve_commit(&repo, branch)?;
    if head.id() == theirs.id() || repo.graph_descendant_of(head.id(), theirs.id())? {
        return Ok(MergePreview {
            up_to_date: true,
            ..MergePreview::default()
        });
    }
    let mut walk = repo.revwalk()?;
    walk.push(theirs.id())?;
    walk.hide(head.id())?;
    let commits = walk.count();
    let fast_forward = repo.graph_descendant_of(theirs.id(), head.id())?;
    let mut conflicts = Vec::new();
    if !fast_forward {
        let index = repo.merge_commits(&head, &theirs, None)?;
        if index.has_conflicts() {
            for conflict in index.conflicts()? {
                let conflict = conflict?;
                let entry = conflict.our.or(conflict.their).or(conflict.ancestor);
                if let Some(entry) = entry {
                    conflicts.push(String::from_utf8_lossy(&entry.path).into_owned());
                }
            }
            conflicts.sort();
            conflicts.dedup();
        }
    }
    Ok(MergePreview {
        commits,
        conflicts,
        up_to_date: false,
        fast_forward,
    })
}

/// Merges `branch` into the current branch. Conflicts leave the merge open
/// (a "merge" operation in the status) for the user to resolve and continue
/// or abort.
pub fn merge_branch(path: &Path, branch: &str) -> Result<MergeOutcome, GitError> {
    {
        let repo = Repository::open(path)?;
        resolve_commit(&repo, branch)?;
        if repo.state() != git2::RepositoryState::Clean {
            return Err(invalid(
                "another operation is unfinished in this repo: continue or abort it first",
            ));
        }
    }
    let out = run(
        path,
        &["merge", "--no-edit", "--no-ff", branch],
        &[],
        Kind::Local,
    )?;
    if out.success {
        return Ok(if out.stdout.contains("Already up to date") {
            MergeOutcome::UpToDate
        } else {
            MergeOutcome::Merged
        });
    }
    let status = repo_status(path)?;
    if status.in_progress == Some("merge") && status.conflict_count() > 0 {
        return Ok(MergeOutcome::Conflicts(status.conflict_count()));
    }
    Err(invalid(out.message()))
}

/// Which unfinished operation `path` is in the middle of.
fn operation(path: &Path) -> Result<&'static str, GitError> {
    repo_status(path)?
        .in_progress
        .filter(|op| matches!(*op, "merge" | "rebase" | "cherry-pick" | "revert"))
        .ok_or_else(|| invalid("nothing is in progress in this repo"))
}

/// Abandons the merge, rebase, cherry-pick or revert in progress and puts the
/// repo back as it was before it started.
pub fn abort_operation(path: &Path) -> Result<(), GitError> {
    let op = operation(path)?;
    run_ok(path, &[op, "--abort"], &[], Kind::Local)?;
    Ok(())
}

/// Finishes the operation in progress once every conflict is resolved:
/// commits a merge, or carries on with a rebase, cherry-pick or revert.
pub fn continue_operation(path: &Path) -> Result<(), GitError> {
    let status = repo_status(path)?;
    let op = operation(path)?;
    if status.conflict_count() > 0 {
        return Err(invalid(format!(
            "{} file{} still conflict{}: resolve them first",
            status.conflict_count(),
            if status.conflict_count() == 1 {
                ""
            } else {
                "s"
            },
            if status.conflict_count() == 1 {
                "s"
            } else {
                ""
            },
        )));
    }
    let out = match op {
        "merge" => run(path, &["commit", "--no-edit"], &[], Kind::Local)?,
        _ => run(path, &[op, "--continue"], &[], Kind::Local)?,
    };
    if out.success {
        return Ok(());
    }
    // A rebase that stops again at the next commit's conflicts is progress,
    // not a failure.
    if repo_status(path)?.conflict_count() > 0 {
        return Ok(());
    }
    Err(invalid(out.message()))
}

/// Skips the commit the rebase, cherry-pick or revert is stuck on.
pub fn skip_commit(path: &Path) -> Result<(), GitError> {
    let op = operation(path)?;
    if !matches!(op, "rebase" | "cherry-pick") {
        return Err(invalid("only a rebase or cherry-pick can skip a commit"));
    }
    let out = run(path, &[op, "--skip"], &[], Kind::Local)?;
    if out.success || repo_status(path)?.conflict_count() > 0 {
        Ok(())
    } else {
        Err(invalid(out.message()))
    }
}

/// Which side of a conflict to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the file as it is on the branch you are on.
    KeepCurrent,
    /// Take the incoming branch's version.
    TakeIncoming,
    /// The file was edited by hand: just record it as resolved.
    MarkResolved,
}

/// Whether the file still contains conflict markers, so "mark resolved" can
/// warn before staging a half-finished merge.
pub fn has_conflict_markers(path: &Path, file: &str) -> Result<bool, GitError> {
    let rel = check_rel_path(file)?;
    let text = std::fs::read(path.join(rel))?;
    let text = String::from_utf8_lossy(&text);
    Ok(text.lines().any(|l| l.starts_with("<<<<<<< "))
        && text.lines().any(|l| l.starts_with(">>>>>>> ")))
}

/// Resolves one conflicted file. During a rebase git swaps "ours" and
/// "theirs" (ours is the branch being rebased onto), which is handled here so
/// "current" always means the branch the user is working on.
pub fn resolve_conflict(path: &Path, file: &str, how: Resolution) -> Result<(), GitError> {
    let rel = check_rel_path(file)?;
    let status = repo_status(path)?;
    if !status
        .files
        .iter()
        .any(|f| f.path == rel && f.is_conflicted())
    {
        return Err(invalid(format!("{rel} isn't in conflict")));
    }
    let rebasing = status.in_progress == Some("rebase");
    let side = match (how, rebasing) {
        (Resolution::MarkResolved, _) => None,
        (Resolution::KeepCurrent, false) | (Resolution::TakeIncoming, true) => Some("--ours"),
        (Resolution::TakeIncoming, false) | (Resolution::KeepCurrent, true) => Some("--theirs"),
    };
    if let Some(side) = side {
        let out = run(path, &["checkout", side, "--", rel], &[], Kind::Local)?;
        if !out.success {
            // The chosen side deleted the file: keeping that side means removing it.
            run_ok(path, &["rm", "--quiet", "--", rel], &[], Kind::Local)?;
            return Ok(());
        }
    }
    run_ok(path, &["add", "--", rel], &[], Kind::Local)?;
    Ok(())
}

/// Creates a commit that undoes `hash` (`git revert`). A conflict leaves the
/// revert open to resolve.
pub fn revert_commit(path: &Path, hash: &str) -> Result<bool, GitError> {
    if !is_full_hash(hash) {
        return Err(invalid("not a commit id"));
    }
    let repo = Repository::open(path)?;
    let oid = git2::Oid::from_str(hash)?;
    let commit = repo.find_commit(oid)?;
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid(
            "another operation is unfinished: continue or abort it first",
        ));
    }
    let mut args = vec!["revert", "--no-edit"];
    // Reverting a merge needs to know which side is the mainline.
    if commit.parent_count() > 1 {
        args.extend(["-m", "1"]);
    }
    args.push(hash);
    let out = run(path, &args, &[], Kind::Local)?;
    if out.success {
        return Ok(false);
    }
    if repo_status(path)?.conflict_count() > 0 {
        return Ok(true);
    }
    Err(invalid(out.message()))
}

/// The commit that "undo last commit" removed, so its message can go back
/// into the commit box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoneCommit {
    pub summary: String,
    pub description: String,
}

/// Moves the branch back one commit and leaves that commit's changes staged
/// (a soft reset), like GitHub Desktop's "Undo". Refuses with nothing to undo
/// or in the middle of another operation.
pub fn undo_last_commit(path: &Path) -> Result<UndoneCommit, GitError> {
    let repo = Repository::open(path)?;
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid("finish or abort the operation in progress first"));
    }
    let head = repo
        .head()
        .map_err(|_| invalid("there is no commit to undo"))?
        .peel_to_commit()?;
    if head.parent_count() == 0 {
        return Err(invalid(
            "this is the first commit; there is nothing before it to go back to",
        ));
    }
    let parent = head.parent(0)?;
    let message = head.message().unwrap_or("").to_owned();
    repo.reset(parent.as_object(), ResetType::Soft, None)?;
    let (summary, description) = match message.split_once('\n') {
        Some((first, rest)) => (first.trim().to_owned(), rest.trim().to_owned()),
        None => (message.trim().to_owned(), String::new()),
    };
    Ok(UndoneCommit {
        summary,
        description,
    })
}

/// Whether the last commit is already on the branch's upstream, so the undo
/// confirmation can say it was pushed.
pub fn last_commit_is_pushed(path: &Path) -> bool {
    repo_status(path)
        .map(|s| s.upstream.is_some() && s.ahead == 0)
        .unwrap_or(false)
}

/// Applies `hashes` (oldest first) onto the current branch. Returns `true`
/// when it stopped on a conflict to resolve.
pub fn cherry_pick(path: &Path, hashes: &[String]) -> Result<bool, GitError> {
    if hashes.is_empty() {
        return Err(invalid("pick at least one commit"));
    }
    if hashes.iter().any(|h| !is_full_hash(h)) {
        return Err(invalid("not a commit id"));
    }
    let repo = Repository::open(path)?;
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid(
            "another operation is unfinished: continue or abort it first",
        ));
    }
    for hash in hashes {
        let commit = repo.find_commit(git2::Oid::from_str(hash)?)?;
        if commit.parent_count() > 1 {
            return Err(invalid("a merge commit can't be cherry-picked"));
        }
    }
    let mut args: Vec<&str> = vec!["cherry-pick"];
    args.extend(hashes.iter().map(String::as_str));
    let out = run(path, &args, &[], Kind::Local)?;
    if out.success {
        return Ok(false);
    }
    if repo_status(path)?.conflict_count() > 0 {
        return Ok(true);
    }
    // An empty pick (already applied) stops git; skip it so the rest go on.
    if out.message().contains("empty") {
        let _ = run(path, &["cherry-pick", "--abort"], &[], Kind::Local);
        return Err(invalid("that change is already in this branch"));
    }
    Err(invalid(out.message()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::git_executable;
    use crate::testutil::Fixture;

    fn git_available() -> bool {
        git_executable().is_some()
    }

    /// `main` plus a `feature` branch that diverged, with `a.txt` edited on
    /// both when `conflict` is set.
    fn diverged(conflict: bool) -> Fixture {
        let f = Fixture::with_files(&[("a.txt", "base\n"), ("keep.txt", "keep\n")]);
        let base = f.branch_name().unwrap();
        f.make_branch("feature");
        crate::switch_branch(f.path(), "feature", crate::SwitchMode::Bring).unwrap();
        f.write(
            if conflict { "a.txt" } else { "feat.txt" },
            "feature side\n",
        );
        f.stage_all();
        f.commit("feature work");
        crate::switch_branch(f.path(), &base, crate::SwitchMode::Bring).unwrap();
        f.write(if conflict { "a.txt" } else { "main.txt" }, "main side\n");
        f.stage_all();
        f.commit("main work");
        f
    }

    #[test]
    fn preview_counts_commits_and_names_conflicts() {
        let clean = diverged(false);
        let p = merge_preview(clean.path(), "feature").unwrap();
        assert_eq!((p.commits, p.conflicts.len(), p.up_to_date), (1, 0, false));

        let bad = diverged(true);
        let p = merge_preview(bad.path(), "feature").unwrap();
        assert_eq!(p.conflicts, ["a.txt"]);
        assert!(merge_preview(bad.path(), "--evil").is_err());
        assert!(merge_preview(bad.path(), "nope").is_err());
    }

    #[test]
    fn preview_recognises_up_to_date_and_fast_forward() {
        let f = Fixture::with_files(&[("a", "1")]);
        let base = f.branch_name().unwrap();
        f.make_branch("ahead");
        crate::switch_branch(f.path(), "ahead", crate::SwitchMode::Bring).unwrap();
        f.write("b", "2");
        f.stage_all();
        f.commit("more");
        crate::switch_branch(f.path(), &base, crate::SwitchMode::Bring).unwrap();
        let p = merge_preview(f.path(), "ahead").unwrap();
        assert!(p.fast_forward && p.commits == 1);
        crate::switch_branch(f.path(), "ahead", crate::SwitchMode::Bring).unwrap();
        assert!(merge_preview(f.path(), &base).unwrap().up_to_date);
    }

    #[test]
    fn a_clean_merge_creates_a_merge_commit() {
        if !git_available() {
            return;
        }
        let f = diverged(false);
        assert_eq!(
            merge_branch(f.path(), "feature").unwrap(),
            MergeOutcome::Merged
        );
        assert!(f.exists("feat.txt") && f.exists("main.txt"));
        assert_eq!(repo_status(f.path()).unwrap().in_progress, None);
        assert_eq!(
            merge_branch(f.path(), "feature").unwrap(),
            MergeOutcome::UpToDate
        );
    }

    #[test]
    fn conflicts_can_be_resolved_either_way_and_the_merge_continued() {
        if !git_available() {
            return;
        }
        let f = diverged(true);
        assert_eq!(
            merge_branch(f.path(), "feature").unwrap(),
            MergeOutcome::Conflicts(1)
        );
        assert!(has_conflict_markers(f.path(), "a.txt").unwrap());
        assert!(
            continue_operation(f.path()).is_err(),
            "can't finish with a conflict"
        );

        resolve_conflict(f.path(), "a.txt", Resolution::TakeIncoming).unwrap();
        assert_eq!(f.read("a.txt"), "feature side\n");
        continue_operation(f.path()).unwrap();
        assert_eq!(repo_status(f.path()).unwrap().in_progress, None);
    }

    #[test]
    fn keep_current_and_hand_edits_resolve_too() {
        if !git_available() {
            return;
        }
        let f = diverged(true);
        merge_branch(f.path(), "feature").unwrap();
        resolve_conflict(f.path(), "a.txt", Resolution::KeepCurrent).unwrap();
        assert_eq!(f.read("a.txt"), "main side\n");
        abort_operation(f.path()).unwrap();

        merge_branch(f.path(), "feature").unwrap();
        f.write("a.txt", "both\n");
        assert!(!has_conflict_markers(f.path(), "a.txt").unwrap());
        resolve_conflict(f.path(), "a.txt", Resolution::MarkResolved).unwrap();
        continue_operation(f.path()).unwrap();
        assert_eq!(f.read("a.txt"), "both\n");
        assert!(resolve_conflict(f.path(), "a.txt", Resolution::KeepCurrent).is_err());
    }

    #[test]
    fn abort_restores_the_branch() {
        if !git_available() {
            return;
        }
        let f = diverged(true);
        merge_branch(f.path(), "feature").unwrap();
        abort_operation(f.path()).unwrap();
        assert_eq!(f.read("a.txt"), "main side\n");
        assert_eq!(repo_status(f.path()).unwrap().in_progress, None);
        assert!(abort_operation(f.path()).is_err());
    }

    #[test]
    fn a_file_deleted_on_one_side_can_be_kept_deleted() {
        if !git_available() {
            return;
        }
        let f = Fixture::with_files(&[("a.txt", "base\n")]);
        let base = f.branch_name().unwrap();
        f.make_branch("feature");
        crate::switch_branch(f.path(), "feature", crate::SwitchMode::Bring).unwrap();
        std::fs::remove_file(f.path().join("a.txt")).unwrap();
        f.stage_all();
        f.commit("delete");
        crate::switch_branch(f.path(), &base, crate::SwitchMode::Bring).unwrap();
        f.write("a.txt", "edited\n");
        f.stage_all();
        f.commit("edit");
        assert_eq!(
            merge_branch(f.path(), "feature").unwrap(),
            MergeOutcome::Conflicts(1)
        );
        resolve_conflict(f.path(), "a.txt", Resolution::TakeIncoming).unwrap();
        assert!(!f.exists("a.txt"));
        continue_operation(f.path()).unwrap();
    }

    #[test]
    fn revert_adds_an_undoing_commit() {
        if !git_available() {
            return;
        }
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        f.write("a.txt", "two\n");
        f.stage_all();
        let oid = f.commit("change");
        assert!(!revert_commit(f.path(), &oid.to_string()).unwrap());
        assert_eq!(f.read("a.txt"), "one\n");
        assert!(revert_commit(f.path(), "HEAD").is_err());
    }

    #[test]
    fn undo_keeps_the_changes_staged_and_returns_the_message() {
        let f = Fixture::with_files(&[("a.txt", "one\n")]);
        f.write("a.txt", "two\n");
        f.stage_all();
        f.commit("Summary line\n\nBody text");
        let undone = undo_last_commit(f.path()).unwrap();
        assert_eq!(undone.summary, "Summary line");
        assert_eq!(undone.description, "Body text");
        assert_eq!(f.index_content("a.txt").as_deref(), Some("two\n"));
        let status = repo_status(f.path()).unwrap();
        assert_eq!(status.staged_count(), 1);
        // The first commit has nothing before it.
        undo_last_commit(f.path()).unwrap_err();
    }

    #[test]
    fn cherry_pick_applies_commits_in_order_and_stops_on_conflict() {
        if !git_available() {
            return;
        }
        let f = Fixture::with_files(&[("a.txt", "base\n")]);
        let base = f.branch_name().unwrap();
        f.make_branch("other");
        crate::switch_branch(f.path(), "other", crate::SwitchMode::Bring).unwrap();
        f.write("x.txt", "x\n");
        f.stage_all();
        let x = f.commit("add x").to_string();
        f.write("y.txt", "y\n");
        f.stage_all();
        let y = f.commit("add y").to_string();
        crate::switch_branch(f.path(), &base, crate::SwitchMode::Bring).unwrap();
        assert!(!cherry_pick(f.path(), &[x, y]).unwrap());
        assert!(f.exists("x.txt") && f.exists("y.txt"));

        // Conflicting pick.
        crate::switch_branch(f.path(), "other", crate::SwitchMode::Bring).unwrap();
        f.write("a.txt", "other\n");
        f.stage_all();
        let c = f.commit("edit a on other").to_string();
        crate::switch_branch(f.path(), &base, crate::SwitchMode::Bring).unwrap();
        f.write("a.txt", "main\n");
        f.stage_all();
        f.commit("edit a on main");
        assert!(
            cherry_pick(f.path(), &[c]).unwrap(),
            "stops on the conflict"
        );
        assert_eq!(
            repo_status(f.path()).unwrap().in_progress,
            Some("cherry-pick")
        );
        abort_operation(f.path()).unwrap();
        assert!(cherry_pick(f.path(), &["not-a-hash".into()]).is_err());
    }
}
