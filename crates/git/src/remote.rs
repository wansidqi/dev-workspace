//! Fetch, pull, push, sync and clone (Phase 2). These go through the git CLI
//! so the user's credential manager, SSH config and hooks are used.

use std::path::Path;

use git2::{BranchType, Repository};

use crate::cli::{run_ok, Kind};
use crate::{is_safe_ref_name, repo_status, GitError};

/// How a pull combines the remote's commits with local ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullMode {
    /// Merge when the branches diverged (git's classic default).
    #[default]
    Merge,
    /// Replay local commits on top of the remote's.
    Rebase,
    /// Refuse unless it is a plain fast-forward.
    FastForwardOnly,
}

/// What a remote action did, for the notice line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOutcome {
    /// One line a person can read ("Fetched origin", "Pushed 2 commits").
    pub message: String,
    /// The action stopped with conflicts to resolve.
    pub conflicts: bool,
}

impl RemoteOutcome {
    fn done(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            conflicts: false,
        }
    }
}

/// The remote a branch talks to: its configured upstream remote, else
/// `origin`, else the only remote. `None` when the repo has no remote.
fn remote_for(repo: &Repository, branch: Option<&str>) -> Option<String> {
    if let Some(branch) = branch {
        let key = format!("refs/heads/{branch}");
        if let Some(name) = repo
            .branch_upstream_remote(&key)
            .ok()
            .and_then(|b| b.as_str().map(str::to_owned))
        {
            return Some(name);
        }
    }
    let remotes = repo.remotes().ok()?;
    let names: Vec<&str> = remotes.iter().flatten().collect();
    if names.contains(&"origin") {
        Some("origin".into())
    } else if names.len() == 1 {
        Some(names[0].to_owned())
    } else {
        None
    }
}

fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.find_reference("HEAD").ok()?;
    head.symbolic_target()
        .and_then(|t| t.strip_prefix("refs/heads/"))
        .map(str::to_owned)
}

/// How the user's git config wants a pull to combine histories: `pull.rebase`
/// true means rebase, `pull.ff=only` means fast-forward only, anything else
/// merges.
pub fn configured_pull_mode(path: &Path) -> PullMode {
    let Ok(config) = Repository::open(path).and_then(|r| r.config()) else {
        return PullMode::Merge;
    };
    if config.get_bool("pull.rebase").unwrap_or(false) {
        PullMode::Rebase
    } else if config.get_string("pull.ff").is_ok_and(|v| v == "only") {
        PullMode::FastForwardOnly
    } else {
        PullMode::Merge
    }
}

/// The address of the remote a branch talks to (`origin` by default), for
/// working out which host a repo lives on. Callers must not log it: it can
/// carry credentials.
pub fn remote_url(path: &Path) -> Option<String> {
    let repo = Repository::open(path).ok()?;
    let name = remote_for(&repo, current_branch(&repo).as_deref())?;
    let remote = repo.find_remote(&name).ok()?;
    remote.url().map(str::to_owned)
}

/// The branch pull requests usually target: the remote's default branch,
/// else `main` / `master` / `develop` when they exist locally.
pub fn default_base_branch(path: &Path) -> Option<String> {
    let target = crate::default_target_branch(path)?;
    Some(target.strip_prefix("origin/").unwrap_or(&target).to_owned())
}

/// Names of the repo's remotes.
pub fn remote_names(path: &Path) -> Result<Vec<String>, GitError> {
    let repo = Repository::open(path)?;
    let remotes = repo.remotes()?;
    Ok(remotes.iter().flatten().map(str::to_owned).collect())
}

fn require_remote(path: &Path) -> Result<(Repository, Option<String>, String), GitError> {
    let repo = Repository::open(path)?;
    let branch = current_branch(&repo);
    let remote = remote_for(&repo, branch.as_deref()).ok_or_else(|| {
        GitError::Invalid(
            "this repo has no remote to talk to (add one with `git remote add`)".into(),
        )
    })?;
    if !is_safe_ref_name(&remote) {
        return Err(GitError::Invalid(format!(
            "unusable remote name `{remote}`"
        )));
    }
    Ok((repo, branch, remote))
}

/// `git fetch --prune` from the current branch's remote.
pub fn fetch(path: &Path) -> Result<RemoteOutcome, GitError> {
    let (_, _, remote) = require_remote(path)?;
    run_ok(path, &["fetch", "--prune", &remote], &[], Kind::Network)?;
    Ok(RemoteOutcome::done(format!("fetched {remote}")))
}

fn conflicts_after(path: &Path) -> bool {
    repo_status(path)
        .map(|s| s.conflict_count() > 0 || s.in_progress.is_some())
        .unwrap_or(false)
}

/// Fetches, then brings the remote's commits into the current branch.
pub fn pull(path: &Path, mode: PullMode) -> Result<RemoteOutcome, GitError> {
    let (_, branch, remote) = require_remote(path)?;
    let branch = branch
        .ok_or_else(|| GitError::Invalid("HEAD is detached; switch to a branch to pull".into()))?;
    let before = repo_status(path)?;
    if before.upstream.is_none() {
        return Err(GitError::Invalid(format!(
            "`{branch}` isn't published yet, so there is nothing to pull"
        )));
    }
    let flag = match mode {
        PullMode::Merge => "--no-rebase",
        PullMode::Rebase => "--rebase",
        PullMode::FastForwardOnly => "--ff-only",
    };
    let out = crate::cli::run(
        path,
        &["pull", flag, "--no-edit", &remote],
        &[],
        Kind::Network,
    )?;
    if !out.success {
        if conflicts_after(path) {
            return Ok(RemoteOutcome {
                message: format!(
                    "pulled {remote}, but some files conflict: resolve them to finish"
                ),
                conflicts: true,
            });
        }
        return Err(GitError::Invalid(out.message()));
    }
    let text = out.stdout.trim();
    if text.contains("Already up to date") {
        Ok(RemoteOutcome::done("already up to date"))
    } else {
        Ok(RemoteOutcome::done(format!(
            "pulled {} new commit{}",
            before.behind,
            if before.behind == 1 { "" } else { "s" }
        )))
    }
}

/// Pushes the current branch. Publishes it (sets the upstream) the first
/// time. `force_with_lease` is the safe force: it refuses if someone else
/// pushed since you last fetched.
pub fn push(path: &Path, force_with_lease: bool) -> Result<RemoteOutcome, GitError> {
    let (repo, branch, remote) = require_remote(path)?;
    let branch = branch
        .ok_or_else(|| GitError::Invalid("HEAD is detached; switch to a branch to push".into()))?;
    if !is_safe_ref_name(&branch) {
        return Err(GitError::Invalid(format!(
            "unusable branch name `{branch}`"
        )));
    }
    let status = repo_status(path)?;
    let upstream_branch = repo
        .find_branch(&branch, BranchType::Local)
        .ok()
        .and_then(|b| b.upstream().ok())
        .and_then(|u| u.name().ok().flatten().map(str::to_owned))
        .and_then(|full| full.split_once('/').map(|(_, rest)| rest.to_owned()));
    let publishing = upstream_branch.is_none();
    let target = upstream_branch.unwrap_or_else(|| branch.clone());
    if !is_safe_ref_name(&target) {
        return Err(GitError::Invalid(format!(
            "unusable branch name `{target}`"
        )));
    }
    let refspec = format!("{branch}:refs/heads/{target}");
    let mut args = vec!["push"];
    if force_with_lease {
        args.push("--force-with-lease");
    }
    if publishing {
        args.push("--set-upstream");
    }
    args.push(&remote);
    args.push(&refspec);
    run_ok(path, &args, &[], Kind::Network)?;
    Ok(RemoteOutcome::done(if publishing {
        format!("published {branch} to {remote}")
    } else if status.ahead == 0 {
        "nothing new to push".to_owned()
    } else {
        format!(
            "pushed {} commit{}",
            status.ahead,
            if status.ahead == 1 { "" } else { "s" }
        )
    }))
}

/// What the one Sync button does for a repo in its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStep {
    /// Not published: push with `--set-upstream`.
    Publish,
    /// Remote has commits we lack.
    Pull,
    /// We have commits the remote lacks.
    Push,
    /// Nothing known to move: just look for news.
    Fetch,
}

/// The next step for a repo, given what its status says. Pull comes before
/// push, as in GitHub Desktop, so a diverged branch is merged before it is
/// pushed.
pub fn next_sync_step(upstream: bool, ahead: usize, behind: usize, has_remote: bool) -> SyncStep {
    if !has_remote {
        SyncStep::Fetch
    } else if !upstream {
        SyncStep::Publish
    } else if behind > 0 {
        SyncStep::Pull
    } else if ahead > 0 {
        SyncStep::Push
    } else {
        SyncStep::Fetch
    }
}

/// Whether the repo at `path` has any remote at all.
pub fn has_remote(path: &Path) -> bool {
    Repository::open(path)
        .map(|r| remote_for(&r, current_branch(&r).as_deref()).is_some())
        .unwrap_or(false)
}

/// The Sync button: fetch, pull what's new, push what's ours. Stops at the
/// first step that fails or leaves conflicts; the steps that worked are kept
/// in the returned message.
pub fn sync(path: &Path, mode: PullMode) -> Result<RemoteOutcome, GitError> {
    let mut steps: Vec<String> = Vec::new();
    steps.push(fetch(path)?.message);
    let status = repo_status(path)?;
    if status.behind > 0 {
        let out = pull(path, mode)?;
        if out.conflicts {
            return Ok(out);
        }
        steps.push(out.message);
    }
    let status = repo_status(path)?;
    if status.upstream.is_none() || status.ahead > 0 {
        steps.push(push(path, false)?.message);
    }
    Ok(RemoteOutcome::done(steps.join(", ")))
}

/// Folder name for a clone of `url`: the last path segment without `.git`.
pub fn clone_folder_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|c| c.is_control() || "<>:\"|?*".contains(c));
    ok.then(|| name.to_owned())
}

/// Whether `url` can be handed to `git clone` safely: not an option, and
/// either a known scheme, an scp-style `user@host:path`, or a local path.
pub fn is_clonable_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') || url.chars().any(|c| c.is_control()) {
        return false;
    }
    // `ext::` and similar transports can run commands.
    if url.contains("::") {
        return false;
    }
    true
}

/// Clones `url` into `parent/<name>` and returns the new folder. The folder
/// must not exist yet, so a clone never lands in someone's existing files.
pub fn clone_repo(
    url: &str,
    parent: &Path,
    name: Option<&str>,
) -> Result<std::path::PathBuf, GitError> {
    let url = url.trim();
    if !is_clonable_url(url) {
        return Err(GitError::Invalid(
            "that isn't a repository address git can clone".into(),
        ));
    }
    let folder = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => clone_folder_name(n),
        None => clone_folder_name(url),
    }
    .ok_or_else(|| GitError::Invalid("couldn't work out a folder name for the clone".into()))?;
    if !parent.is_dir() {
        return Err(GitError::Invalid(format!(
            "{} isn't a folder",
            parent.display()
        )));
    }
    let dest = parent.join(&folder);
    if dest.exists() {
        return Err(GitError::Invalid(format!(
            "{} already exists",
            dest.display()
        )));
    }
    let dest_str = dest
        .to_str()
        .ok_or_else(|| GitError::Invalid("the folder path isn't valid text".into()))?;
    run_ok(parent, &["clone", "--", url, dest_str], &[], Kind::Network)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::git_executable;
    use crate::testutil::Fixture;

    /// A working repo with a bare `origin` it has pushed to, or `None` when
    /// git isn't installed (CLI tests are skipped then).
    struct Remote {
        _bare: tempfile::TempDir,
        work: Fixture,
    }

    fn setup() -> Option<Remote> {
        git_executable()?;
        let bare = tempfile::tempdir().unwrap();
        Repository::init_bare(bare.path()).unwrap();
        let work = Fixture::with_files(&[("a.txt", "one\n")]);
        let url = bare.path().to_str().unwrap().to_owned();
        work.repo.remote("origin", &url).unwrap();
        // Make `master`/`main` whatever the fixture used and publish it.
        let out = push(work.path(), false).unwrap();
        assert!(out.message.starts_with("published"), "{}", out.message);
        Some(Remote { _bare: bare, work })
    }

    fn clone_of(remote: &Remote) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let url = remote._bare.path().to_str().unwrap().to_owned();
        let dest = clone_repo(&url, dir.path(), Some("copy")).unwrap();
        let repo = Repository::open(&dest).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Other").unwrap();
        cfg.set_str("user.email", "other@example.com").unwrap();
        cfg.set_str("core.autocrlf", "false").unwrap();
        (dir, dest)
    }

    fn commit_file(path: &Path, rel: &str, content: &str, msg: &str) {
        std::fs::write(path.join(rel), content).unwrap();
        run_ok(path, &["add", "--", rel], &[], Kind::Local).unwrap();
        run_ok(path, &["commit", "-q", "-m", msg], &[], Kind::Local).unwrap();
    }

    #[test]
    fn first_push_publishes_and_later_pushes_report_the_count() {
        let Some(r) = setup() else { return };
        r.work.write("a.txt", "two\n");
        r.work.stage_all();
        r.work.commit("second");
        assert_eq!(repo_status(r.work.path()).unwrap().ahead, 1);
        let out = push(r.work.path(), false).unwrap();
        assert_eq!(out.message, "pushed 1 commit");
        assert_eq!(repo_status(r.work.path()).unwrap().ahead, 0);
    }

    #[test]
    fn fetch_then_pull_brings_in_the_other_clones_commit() {
        let Some(r) = setup() else { return };
        let (_keep, other) = clone_of(&r);
        commit_file(&other, "b.txt", "b\n", "from other");
        push(&other, false).unwrap();

        fetch(r.work.path()).unwrap();
        let status = repo_status(r.work.path()).unwrap();
        assert_eq!(status.behind, 1);
        let out = pull(r.work.path(), PullMode::FastForwardOnly).unwrap();
        assert_eq!(out.message, "pulled 1 new commit");
        assert!(r.work.exists("b.txt"));
        assert_eq!(
            pull(r.work.path(), PullMode::Merge).unwrap().message,
            "already up to date"
        );
    }

    #[test]
    fn ff_only_refuses_a_diverged_branch_and_merge_pull_conflicts_are_reported() {
        let Some(r) = setup() else { return };
        let (_keep, other) = clone_of(&r);
        commit_file(&other, "a.txt", "theirs\n", "theirs");
        push(&other, false).unwrap();

        r.work.write("a.txt", "mine\n");
        r.work.stage_all();
        r.work.commit("mine");
        fetch(r.work.path()).unwrap();
        assert!(pull(r.work.path(), PullMode::FastForwardOnly).is_err());

        let out = pull(r.work.path(), PullMode::Merge).unwrap();
        assert!(out.conflicts, "{}", out.message);
        let status = repo_status(r.work.path()).unwrap();
        assert_eq!(status.in_progress, Some("merge"));
        assert_eq!(status.conflict_count(), 1);
    }

    #[test]
    fn sync_fetches_pulls_and_pushes() {
        let Some(r) = setup() else { return };
        let (_keep, other) = clone_of(&r);
        commit_file(&other, "b.txt", "b\n", "from other");
        push(&other, false).unwrap();
        r.work.write("c.txt", "c\n");
        r.work.stage_all();
        r.work.commit("local");

        let out = sync(r.work.path(), PullMode::Merge).unwrap();
        assert!(!out.conflicts, "{}", out.message);
        let status = repo_status(r.work.path()).unwrap();
        assert_eq!((status.ahead, status.behind), (0, 0));
        assert!(r.work.exists("b.txt") && r.work.exists("c.txt"));
    }

    #[test]
    fn force_with_lease_refuses_when_the_remote_moved() {
        let Some(r) = setup() else { return };
        let (_keep, other) = clone_of(&r);
        commit_file(&other, "b.txt", "b\n", "from other");
        push(&other, false).unwrap();
        // We haven't fetched, so our lease is stale.
        r.work.write("a.txt", "x\n");
        r.work.stage_all();
        r.work.commit("local");
        assert!(push(r.work.path(), false).is_err());
        assert!(push(r.work.path(), true).is_err());
    }

    #[test]
    fn a_repo_without_a_remote_explains_itself() {
        if git_executable().is_none() {
            return;
        }
        let f = Fixture::with_files(&[("a", "1")]);
        let err = fetch(f.path()).unwrap_err().to_string();
        assert!(err.contains("no remote"), "{err}");
        assert!(!has_remote(f.path()));
    }

    #[test]
    fn the_sync_button_picks_the_right_step() {
        assert_eq!(next_sync_step(false, 0, 0, true), SyncStep::Publish);
        assert_eq!(next_sync_step(true, 2, 3, true), SyncStep::Pull);
        assert_eq!(next_sync_step(true, 2, 0, true), SyncStep::Push);
        assert_eq!(next_sync_step(true, 0, 0, true), SyncStep::Fetch);
        assert_eq!(next_sync_step(false, 0, 0, false), SyncStep::Fetch);
    }

    #[test]
    fn clone_names_and_urls() {
        assert_eq!(
            clone_folder_name("https://github.com/a/b.git").as_deref(),
            Some("b")
        );
        assert_eq!(
            clone_folder_name("git@github.com:a/tool.git/").as_deref(),
            Some("tool")
        );
        assert_eq!(clone_folder_name("C:\\src\\proj").as_deref(), Some("proj"));
        assert_eq!(clone_folder_name("--evil"), None);
        assert!(is_clonable_url("https://github.com/a/b.git"));
        assert!(!is_clonable_url("--upload-pack=calc"));
        assert!(!is_clonable_url("ext::sh -c calc"));
        assert!(!is_clonable_url(""));
    }

    #[test]
    fn clone_refuses_an_existing_folder() {
        let Some(r) = setup() else { return };
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("copy")).unwrap();
        let url = r._bare.path().to_str().unwrap().to_owned();
        let err = clone_repo(&url, dir.path(), Some("copy"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"), "{err}");
    }
}
