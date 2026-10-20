//! Thin wrapper around `git2` for commit history, patch-id matching and the
//! local working-copy client (status, diffs, staging, commit, branches,
//! stash). The git CLI is reserved for network and history-rewriting work
//! (see CLAUDE.md "Tech stack"); everything here uses libgit2 directly.

use devspace_core::{Repo, RepoId, TaskId};
use git2::Repository;

mod branches;
mod changes;
mod commit;
mod diff;
mod stash;
#[cfg(test)]
mod testutil;
mod timesheet;

pub use branches::{
    create_branch, delete_branch, rename_branch, switch_branch, SwitchMode, SwitchOutcome,
};
pub use changes::{
    discard_files, repo_status, stage_files, unstage_files, ChangeKind, ChangedFile, RepoStatus,
};
pub use commit::{commit_staged, last_commit_message, CommitRequest};
pub use diff::{
    discard_lines, file_diff, stage_lines, unstage_lines, DiffHunk, DiffLine, DiffSide, FileDiff,
    LineKind, LineSelection,
};
pub use stash::{stash_apply, stash_drop, stash_list, stash_pop, stash_save, StashEntry};
pub use timesheet::timesheet_commits;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    /// A request git can't honour as given (bad name, nothing staged, a
    /// merge in progress...). The text is shown to the user as is.
    #[error("{0}")]
    Invalid(String),
    /// A branch delete that needs the caller's explicit go-ahead.
    #[error("branch `{0}` has commits that are not merged into the current branch")]
    NotMerged(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A path relative to the repo root, as git reports it: no absolute paths,
/// drive prefixes or `..` components. Everything that touches the working
/// tree by name goes through this first (CLAUDE.md path rule).
pub(crate) fn check_rel_path(path: &str) -> Result<&str, GitError> {
    let ok = !path.is_empty()
        && !path.contains('\0')
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if ok {
        Ok(path)
    } else {
        Err(GitError::Invalid(format!(
            "not a path inside the repo: {path}"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub patch_id: String,
    pub summary: String,
    pub author: String,
    pub time: i64,
}

/// Opens a repo at `path` and returns up to `limit` commits reachable from
/// HEAD, newest first. Paged rather than loading full history at once
/// (CLAUDE.md performance rule).
pub fn recent_commits(path: &std::path::Path, limit: usize) -> Result<Vec<CommitInfo>, GitError> {
    let repo = Repository::open(path)?;
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;

    let mut out = Vec::with_capacity(limit);
    for oid in revwalk.take(limit) {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let patch_id = repo.find_commit(oid)?.tree_id();
        out.push(CommitInfo {
            hash: oid.to_string(),
            patch_id: patch_id.to_string(),
            summary: commit.summary().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            time: commit.time().seconds(),
        });
    }
    Ok(out)
}

/// The working-tree root of the git repo containing `path` (which may be a
/// subfolder), or `None` if `path` isn't inside a repo. Lets the folder
/// picker link the repo even when the user selects a subdirectory, and lets
/// the UI tell "not a git repo" apart at link time.
pub fn repo_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let repo = Repository::discover(path).ok()?;
    repo.workdir().map(|p| p.to_path_buf())
}

/// Looks up specific commits by `(repo, hash)` — what a task's linked
/// commits need, since links store only the identity and the message, author
/// and date are read from git at display time (CLAUDE.md). A commit that
/// can't be found (rebased or squashed away, repo moved) maps to `None` so
/// the UI can warn instead of showing stale data.
pub fn lookup_commits(
    repos: &[Repo],
    keys: &[(RepoId, String)],
) -> Vec<((RepoId, String), Option<RepoCommit>)> {
    let mut opened: std::collections::HashMap<RepoId, Option<Repository>> =
        std::collections::HashMap::new();
    keys.iter()
        .map(|(repo_id, hash)| {
            let found = repos.iter().find(|r| r.id == *repo_id).and_then(|repo| {
                let git = opened
                    .entry(*repo_id)
                    .or_insert_with(|| Repository::open(&repo.path).ok())
                    .as_ref()?;
                let oid = git2::Oid::from_str(hash).ok()?;
                let commit = git.find_commit(oid).ok()?;
                let entry = RepoCommit {
                    repo_id: repo.id,
                    repo_name: repo.name.clone(),
                    commit: CommitInfo {
                        hash: commit.id().to_string(),
                        patch_id: commit.tree_id().to_string(),
                        summary: commit.summary().unwrap_or("").to_string(),
                        author: commit.author().name().unwrap_or("").to_string(),
                        time: commit.time().seconds(),
                    },
                };
                Some(entry)
            });
            ((*repo_id, hash.clone()), found)
        })
        .collect()
}

/// A commit tagged with the linked repo it came from. A hash is only unique
/// within one repo, so `(repo_id, commit.hash)` is the commit's identity
/// (CLAUDE.md storage rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCommit {
    pub repo_id: RepoId,
    pub repo_name: String,
    pub commit: CommitInfo,
}

/// Position in the merged history, for paging: the last commit of the
/// previous page. The next page holds everything strictly after it in
/// (newest time, repo id, hash) order, so same-second commits and
/// same-hash-prefix commits in different repos neither repeat nor vanish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCursor {
    pub time: i64,
    pub repo_id: RepoId,
    pub hash: String,
}

impl HistoryCursor {
    fn of(c: &RepoCommit) -> Self {
        Self {
            time: c.commit.time,
            repo_id: c.repo_id,
            hash: c.commit.hash.clone(),
        }
    }
}

/// One page of merged history across a project's repos.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryPage {
    pub commits: Vec<RepoCommit>,
    /// Pass to the next call for the following page; `None` when this page
    /// was the last one.
    pub next: Option<HistoryCursor>,
    /// Repos that couldn't be read (moved, deleted, not a git repo). One bad
    /// repo must not hide the others' history, so they're reported here for
    /// the UI to flag instead of failing the whole page.
    pub unreadable: Vec<RepoId>,
    /// Repos that were readable but lack the requested branch (or the branch
    /// name was unusable). Not an error: a project's repos rarely share every
    /// branch, so the UI shows "not in: <repo>" and the rest still list.
    pub missing_branch: Vec<RepoId>,
}

/// Optional narrowing of the merged history. All fields are conjunctive and
/// the default matches everything, so existing callers are unaffected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    /// Case-insensitive match on title, author or hash prefix.
    pub text: String,
    /// Read this branch (local first, then `origin/<name>`) instead of HEAD.
    pub branch: Option<String>,
    /// Case-insensitive substring of the author's name.
    pub author: Option<String>,
    /// Inclusive lower bound on commit time (unix seconds).
    pub since: Option<i64>,
    /// Inclusive upper bound on commit time (unix seconds). Callers wanting
    /// "through the whole day" pass the day's last second.
    pub until: Option<i64>,
}

/// A [`HistoryFilter`] normalised once per call instead of once per commit.
struct Prepared {
    needle: String,
    branch: Option<String>,
    author: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
}

impl Prepared {
    fn new(f: &HistoryFilter) -> Self {
        let nonblank = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            needle: f.text.trim().to_lowercase(),
            branch: nonblank(&f.branch),
            author: nonblank(&f.author).map(|a| a.to_lowercase()),
            since: f.since,
            until: f.until,
        }
    }

    fn accepts(&self, commit: &CommitInfo) -> bool {
        self.since.is_none_or(|s| commit.time >= s)
            && self.until.is_none_or(|u| commit.time <= u)
            && self
                .author
                .as_deref()
                .is_none_or(|a| commit.author.to_lowercase().contains(a))
            && matches_search(commit, &self.needle)
    }
}

type SortKey = (std::cmp::Reverse<i64>, RepoId, String);

fn sort_key(c: &RepoCommit) -> SortKey {
    (
        std::cmp::Reverse(c.commit.time),
        c.repo_id,
        c.commit.hash.clone(),
    )
}

/// Reads up to `limit` commits from `repo`'s HEAD (or the filter's branch)
/// that sort after `after` and pass the filter. Newest-first by commit time
/// (libgit2 time sorting; with clock skew inside one repo the order is as git
/// itself reports it). `Ok(None)` means the requested branch doesn't exist in
/// this repo. Once a commit is older than `since` the walk stops: in time
/// order everything after it is older still.
fn repo_page(
    repo: &Repo,
    limit: usize,
    after: Option<&SortKey>,
    filter: &Prepared,
) -> Result<Option<Vec<RepoCommit>>, GitError> {
    let git = Repository::open(&repo.path)?;
    // A freshly `git init`-ed repo has no commits yet: empty, not broken.
    if git.is_empty()? {
        return Ok(filter.branch.is_none().then(Vec::new));
    }
    let mut revwalk = git.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME)?;
    match filter.branch.as_deref() {
        None => revwalk.push_head()?,
        Some(branch) => {
            let tip = is_safe_ref_name(branch)
                .then(|| resolve_branch(&git, branch))
                .flatten();
            match tip {
                Some(oid) => revwalk.push(oid)?,
                None => return Ok(None),
            }
        }
    }

    let mut out = Vec::new();
    for oid in revwalk {
        let commit = git.find_commit(oid?)?;
        let entry = RepoCommit {
            repo_id: repo.id,
            repo_name: repo.name.clone(),
            commit: CommitInfo {
                hash: commit.id().to_string(),
                patch_id: commit.tree_id().to_string(),
                summary: commit.summary().unwrap_or("").to_string(),
                author: commit.author().name().unwrap_or("").to_string(),
                time: commit.time().seconds(),
            },
        };
        if filter.since.is_some_and(|s| entry.commit.time < s) {
            break;
        }
        if after.is_some_and(|key| sort_key(&entry) <= *key) {
            continue;
        }
        if !filter.accepts(&entry.commit) {
            continue;
        }
        out.push(entry);
        if out.len() >= limit {
            break;
        }
    }
    Ok(Some(out))
}

/// Case-insensitive match on a commit's title, author or hash prefix.
/// `needle` must already be lowercase; empty matches everything.
fn matches_search(commit: &CommitInfo, needle: &str) -> bool {
    needle.is_empty()
        || commit.summary.to_lowercase().contains(needle)
        || commit.author.to_lowercase().contains(needle)
        || commit.hash.starts_with(needle)
}

/// Merges the history of every repo linked to a project, newest first, each
/// commit labeled with its repo. Each repo contributes at most `limit`
/// commits per page, which is enough to fill a merged page of `limit`.
pub fn merged_history(repos: &[Repo], limit: usize, after: Option<&HistoryCursor>) -> HistoryPage {
    search_history(repos, limit, after, "")
}

/// [`merged_history`] restricted to commits whose title, author or hash
/// prefix contains `text` (case-insensitive). Walks history from the cursor
/// until `limit` matches are found, so call it off the UI thread.
pub fn search_history(
    repos: &[Repo],
    limit: usize,
    after: Option<&HistoryCursor>,
    text: &str,
) -> HistoryPage {
    let filter = HistoryFilter {
        text: text.to_owned(),
        ..HistoryFilter::default()
    };
    filtered_history(repos, limit, after, &filter)
}

/// The full-featured history entry point: text search plus branch, author and
/// date-range filters, merged newest first across `repos`. Paging uses the
/// same cursor as [`merged_history`] and works unchanged under any filter.
/// Repos lacking the requested branch are listed in
/// [`HistoryPage::missing_branch`] instead of failing. Walks history, so call
/// it off the UI thread.
pub fn filtered_history(
    repos: &[Repo],
    limit: usize,
    after: Option<&HistoryCursor>,
    filter: &HistoryFilter,
) -> HistoryPage {
    let prepared = Prepared::new(filter);
    let after_key = after.map(|c| (std::cmp::Reverse(c.time), c.repo_id, c.hash.clone()));
    let mut page = HistoryPage::default();
    let mut all = Vec::new();
    for repo in repos {
        // One extra per repo so "this repo has more" is detectable: without
        // it a page that ends exactly at the limit would claim to be last.
        match repo_page(repo, limit + 1, after_key.as_ref(), &prepared) {
            Ok(Some(commits)) => all.extend(commits),
            Ok(None) => page.missing_branch.push(repo.id),
            Err(_) => page.unreadable.push(repo.id),
        }
    }
    all.sort_by_key(sort_key);
    let more = all.len() > limit;
    all.truncate(limit);
    page.next = more.then(|| all.last().map(HistoryCursor::of)).flatten();
    page.commits = all;
    page
}

/// One repo's branches, plus the checked-out branch if there is one.
fn branches_and_current(repo: &Repo) -> Result<(Vec<String>, Option<String>), GitError> {
    let git = Repository::open(&repo.path)?;
    let current = git
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().map(str::to_owned));
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for entry in git.branches(None)? {
        let (branch, kind) = entry?;
        let Some(name) = branch.name()?.map(str::to_owned) else {
            continue;
        };
        match kind {
            git2::BranchType::Local => local.push(name),
            git2::BranchType::Remote => {
                if let Some(short) = name.strip_prefix("origin/") {
                    if short != "HEAD" {
                        remote.push(short.to_owned());
                    }
                }
            }
        }
    }
    local.sort();
    local.dedup();
    remote.sort();
    remote.dedup();
    remote.retain(|r| local.binary_search(r).is_err());
    let current = current.filter(|c| local.contains(c));
    if let Some(cur) = &current {
        local.retain(|b| b != cur);
    }
    let mut out = Vec::with_capacity(local.len() + remote.len() + 1);
    out.extend(current.clone());
    out.extend(local);
    out.extend(remote);
    Ok((out, current))
}

/// Branch names of one repo: the checked-out branch first, then the other
/// local branches, then `origin/` branches without a local twin (named
/// without the prefix, as [`filtered_history`] resolves them), each group
/// sorted. Pure git2 reads.
pub fn list_branches(repo: &Repo) -> Result<Vec<String>, GitError> {
    branches_and_current(repo).map(|(branches, _)| branches)
}

/// The union of every readable repo's branches, deduplicated by name. Names
/// checked out in some repo come first, then alphabetical, so the common
/// working branches sit at the top of a picker. Unreadable repos are skipped.
pub fn project_branches(repos: &[Repo]) -> Vec<String> {
    let mut checked_out = std::collections::HashSet::new();
    let mut all = std::collections::BTreeSet::new();
    for repo in repos {
        if let Ok((branches, current)) = branches_and_current(repo) {
            checked_out.extend(current);
            all.extend(branches);
        }
    }
    let mut out: Vec<String> = all.into_iter().collect();
    // Stable sort keeps the alphabetical order inside each group.
    out.sort_by_key(|b| !checked_out.contains(b));
    out
}

/// True when `c` can be part of an identifier-like run. Task references
/// only care about alphanumerics and `_`; branch names also contain `-` and
/// `/`, so those count as part of a longer name there.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_branch_char(c: char) -> bool {
    is_word_char(c) || c == '-' || c == '/'
}

/// Case-insensitive search for `needle` in `haystack` that only counts a hit
/// not glued to a longer identifier run on either side, so `#4` never
/// matches inside `#42` and branch `login` never matches `relogin`. Works on
/// lowercased copies of both so non-ASCII text can't split a char boundary.
fn contains_token(haystack: &str, needle: &str, is_ident: fn(char) -> bool) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hay = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    hay.match_indices(&needle).any(|(i, m)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + m.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Whether a commit summary refers to the task (`#<id>`) or, when given a
/// non-blank branch name, mentions that branch, both on token boundaries.
fn summary_mentions_task(summary: &str, task_id: TaskId, branch: Option<&str>) -> bool {
    contains_token(summary, &format!("#{}", task_id.0), is_word_char)
        || branch
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .is_some_and(|b| contains_token(summary, b, is_branch_char))
}

/// Returns the commits whose summary mentions the task id (`#<id>`) or
/// branch name as a whole token — used to surface "suggested commits" for a
/// task that hasn't had anything linked yet.
pub fn suggest_commits_for_task(
    commits: &[CommitInfo],
    task_id: TaskId,
    branch: Option<&str>,
) -> Vec<CommitInfo> {
    commits
        .iter()
        .filter(|c| summary_mentions_task(&c.summary, task_id, branch))
        .cloned()
        .collect()
}

/// Suggests commits across a project's repos that probably belong to a task
/// with nothing linked yet: the summary mentions `#<task_id>` or the task's
/// branch name. Only the newest `scan_limit` commits of each repo are
/// examined (never unbounded history), commits in `already_linked` are
/// skipped, and the result is newest first, at most `max_results`.
pub fn suggested_for_task(
    repos: &[Repo],
    task_id: TaskId,
    branch: Option<&str>,
    already_linked: &std::collections::HashSet<(RepoId, String)>,
    scan_limit: usize,
    max_results: usize,
) -> Vec<RepoCommit> {
    if max_results == 0 || scan_limit == 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for repo in repos {
        // Per repo, so one busy repo can't use up another's scan budget.
        let page = merged_history(std::slice::from_ref(repo), scan_limit, None);
        found.extend(page.commits.into_iter().filter(|c| {
            !already_linked.contains(&(c.repo_id, c.commit.hash.clone()))
                && summary_mentions_task(&c.commit.summary, task_id, branch)
        }));
    }
    found.sort_by_key(sort_key);
    found.truncate(max_results);
    found
}

/// Commits on a task's branch that are not in the target branch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchView {
    /// False when the branch exists neither locally nor as `origin/<branch>`
    /// (or its name is unusable). Normal for a task whose branch was never
    /// created, so it is a state, not an error.
    pub branch_found: bool,
    /// The target actually compared against; `None` when none could be
    /// resolved and the branch's commits are shown unfiltered.
    pub target: Option<String>,
    /// Newest first.
    pub commits: Vec<CommitInfo>,
    /// More commits existed beyond `limit`.
    pub truncated: bool,
}

/// Branch and target names are user-entered; refuse anything git would read
/// as an option or that carries whitespace/control characters.
pub(crate) fn is_safe_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn commit_oid_of(git: &Repository, spec: &str) -> Option<git2::Oid> {
    let obj = git.revparse_single(spec).ok()?;
    Some(obj.peel_to_commit().ok()?.id())
}

fn resolve_branch(git: &Repository, branch: &str) -> Option<git2::Oid> {
    let local = git
        .find_branch(branch, git2::BranchType::Local)
        .ok()
        .and_then(|b| b.get().peel_to_commit().ok());
    local
        .or_else(|| {
            git.find_reference(&format!("refs/remotes/origin/{branch}"))
                .ok()
                .and_then(|r| r.peel_to_commit().ok())
        })
        .map(|c| c.id())
}

fn default_target_in(git: &Repository) -> Option<String> {
    if let Some(name) = git
        .find_reference("refs/remotes/origin/HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().map(str::to_owned))
        .and_then(|t| t.strip_prefix("refs/remotes/").map(str::to_owned))
    {
        if commit_oid_of(git, &name).is_some() {
            return Some(name);
        }
    }
    for base in ["main", "master", "develop"] {
        if git.find_branch(base, git2::BranchType::Local).is_ok() {
            return Some(base.to_owned());
        }
        let remote = format!("origin/{base}");
        if git.find_branch(&remote, git2::BranchType::Remote).is_ok() {
            return Some(remote);
        }
    }
    None
}

/// The branch tasks are normally merged into: the symbolic target of
/// `origin/HEAD`, else `main`, `master` or `develop` (local first, then
/// `origin/`). Returned as a name `revparse_single` accepts; `None` if the
/// repo can't be opened or has none of them.
pub fn default_target_branch(repo_path: &std::path::Path) -> Option<String> {
    default_target_in(&Repository::open(repo_path).ok()?)
}

/// Commits reachable from `branch` but not from the target (`target..branch`),
/// newest first, at most `limit`. `target: None` uses
/// [`default_target_branch`]; when no target resolves (including an explicit
/// one that doesn't exist or has an unsafe name) the branch's commits are
/// returned unfiltered with `target: None`. An unsafe or missing branch name
/// yields `branch_found: false` rather than an error. Git2 only, no shelling
/// out.
pub fn branch_only_commits(
    repo: &Repo,
    branch: &str,
    target: Option<&str>,
    limit: usize,
) -> Result<BranchView, GitError> {
    let git = Repository::open(&repo.path)?;
    let branch = branch.trim();
    let tip = if is_safe_ref_name(branch) {
        resolve_branch(&git, branch)
    } else {
        None
    };
    let Some(tip) = tip else {
        return Ok(BranchView::default());
    };

    let target_name = match target.map(str::trim) {
        Some(t) if is_safe_ref_name(t) => Some(t.to_owned()),
        Some(_) => None,
        None => default_target_in(&git),
    };
    let resolved = target_name.and_then(|name| commit_oid_of(&git, &name).map(|oid| (name, oid)));

    let mut walk = git.revwalk()?;
    walk.set_sorting(git2::Sort::TIME)?;
    walk.push(tip)?;
    if let Some((_, oid)) = &resolved {
        walk.hide(*oid)?;
    }

    let mut commits = Vec::new();
    let mut truncated = false;
    for oid in walk {
        if commits.len() >= limit {
            truncated = true;
            break;
        }
        let commit = git.find_commit(oid?)?;
        commits.push(CommitInfo {
            hash: commit.id().to_string(),
            patch_id: commit.tree_id().to_string(),
            summary: commit.summary().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            time: commit.time().seconds(),
        });
    }
    Ok(BranchView {
        branch_found: true,
        target: resolved.map(|(name, _)| name),
        commits,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo_with_commit(message: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("git must be installed to run this test");
            assert!(status.success());
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", message]);
        dir
    }

    fn git_in(dir: &std::path::Path, args: &[&str], time: Option<i64>) {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        if let Some(t) = time {
            let date = format!("@{t} +0000");
            cmd.env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date);
        }
        assert!(cmd.status().expect("git must be installed").success());
    }

    /// Empty repo (git init + identity), no commits.
    fn init_empty_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        git_in(dir.path(), &["init", "-q"], None);
        git_in(dir.path(), &["config", "user.email", "t@example.com"], None);
        git_in(dir.path(), &["config", "user.name", "Test"], None);
        dir
    }

    /// Adds one commit with a fixed timestamp so ordering is deterministic.
    fn commit_at(dir: &std::path::Path, message: &str, time: i64) {
        std::fs::write(dir.join(format!("{time}.txt")), message).unwrap();
        git_in(dir, &["add", "."], None);
        git_in(dir, &["commit", "-q", "-m", message], Some(time));
    }

    fn repo_of(id: i64, name: &str, dir: &tempfile::TempDir) -> Repo {
        Repo::new(
            RepoId(id),
            devspace_core::ProjectId(1),
            dir.path().to_str().unwrap(),
            name,
        )
        .unwrap()
    }

    fn summaries(page: &HistoryPage) -> Vec<(&str, &str)> {
        page.commits
            .iter()
            .map(|c| (c.repo_name.as_str(), c.commit.summary.as_str()))
            .collect()
    }

    #[test]
    fn lookup_finds_commits_and_reports_missing_ones() {
        let a = init_empty_repo();
        commit_at(a.path(), "real commit", 100);
        let repos = [repo_of(1, "a", &a)];
        let hash = merged_history(&repos, 5, None).commits[0]
            .commit
            .hash
            .clone();
        let gone = "0123456789abcdef0123456789abcdef01234567".to_string();

        let found = lookup_commits(
            &repos,
            &[
                (RepoId(1), hash.clone()),
                (RepoId(1), gone.clone()),
                (RepoId(9), hash.clone()), // repo no longer linked
                (RepoId(1), "not-a-hash".to_string()),
            ],
        );
        assert_eq!(found[0].1.as_ref().unwrap().commit.summary, "real commit");
        assert_eq!(found[0].1.as_ref().unwrap().repo_name, "a");
        assert!(found[1].1.is_none(), "unknown hash is reported missing");
        assert!(found[2].1.is_none(), "unlinked repo is reported missing");
        assert!(found[3].1.is_none(), "malformed hash does not panic");
    }

    #[test]
    fn repo_root_finds_the_root_from_a_subfolder_and_rejects_non_repos() {
        let repo = init_empty_repo();
        let sub = repo.path().join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        let root = repo_root(&sub).expect("subfolder is inside a repo");
        assert_eq!(
            root.canonicalize().unwrap(),
            repo.path().canonicalize().unwrap()
        );

        let plain = tempdir().unwrap();
        assert!(repo_root(plain.path()).is_none());
    }

    #[test]
    fn merged_history_interleaves_repos_newest_first_with_labels() {
        let api = init_empty_repo();
        let web = init_empty_repo();
        commit_at(api.path(), "api 1", 1000);
        commit_at(web.path(), "web 1", 2000);
        commit_at(api.path(), "api 2", 3000);
        commit_at(web.path(), "web 2", 4000);
        let repos = [repo_of(1, "api", &api), repo_of(2, "web", &web)];

        let page = merged_history(&repos, 10, None);
        assert_eq!(
            summaries(&page),
            vec![
                ("web", "web 2"),
                ("api", "api 2"),
                ("web", "web 1"),
                ("api", "api 1")
            ]
        );
        assert!(page.next.is_none());
        assert!(page.unreadable.is_empty());
    }

    #[test]
    fn merged_history_pages_without_gaps_or_repeats() {
        let a = init_empty_repo();
        let b = init_empty_repo();
        // Same-second commits across repos exercise the tie-break.
        for t in [100, 200, 300, 400, 500] {
            commit_at(a.path(), &format!("a{t}"), t);
            commit_at(b.path(), &format!("b{t}"), t);
        }
        let repos = [repo_of(1, "a", &a), repo_of(2, "b", &b)];

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = merged_history(&repos, 3, cursor.as_ref());
            seen.extend(
                page.commits
                    .iter()
                    .map(|c| (c.repo_id, c.commit.hash.clone())),
            );
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen.len(), 10, "every commit exactly once");
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), 10);
        let all = merged_history(&repos, 100, None);
        let expected: Vec<_> = all
            .commits
            .iter()
            .map(|c| (c.repo_id, c.commit.hash.clone()))
            .collect();
        assert_eq!(seen, expected, "paged order matches the unpaged order");
    }

    #[test]
    fn a_page_ending_exactly_at_the_limit_still_offers_the_next_page() {
        // Regression: 6 commits, page size 5 must not report "last page".
        let a = init_empty_repo();
        for t in 1..=6 {
            commit_at(a.path(), &format!("c{t}"), t * 100);
        }
        let repos = [repo_of(1, "a", &a)];
        let first = merged_history(&repos, 5, None);
        assert_eq!(first.commits.len(), 5);
        let second = merged_history(&repos, 5, first.next.as_ref());
        assert_eq!(summaries(&second), vec![("a", "c1")]);
        assert!(second.next.is_none());

        // And exactly `limit` commits really is the last page.
        let exact = merged_history(&repos, 6, None);
        assert_eq!(exact.commits.len(), 6);
        assert!(exact.next.is_none());
    }

    #[test]
    fn search_matches_title_author_and_hash_across_repos_and_pages() {
        let a = init_empty_repo();
        let b = init_empty_repo();
        commit_at(a.path(), "Fix login bug", 100);
        commit_at(b.path(), "Add invoice export", 200);
        commit_at(a.path(), "fix logout redirect", 300);
        commit_at(b.path(), "Refactor", 400);
        let repos = [repo_of(1, "a", &a), repo_of(2, "b", &b)];

        let hits = search_history(&repos, 10, None, "  FIX ");
        assert_eq!(
            summaries(&hits),
            vec![("a", "fix logout redirect"), ("a", "Fix login bug")]
        );
        // Author matches too (all fixtures are authored by "Test").
        assert_eq!(search_history(&repos, 10, None, "test").commits.len(), 4);
        // Hash prefix.
        let hash = hits.commits[0].commit.hash.clone();
        let by_hash = search_history(&repos, 10, None, &hash[..8]);
        assert_eq!(by_hash.commits.len(), 1);
        // Paging works through filtered results.
        let p1 = search_history(&repos, 1, None, "fix");
        let p2 = search_history(&repos, 1, p1.next.as_ref(), "fix");
        assert_eq!(p1.commits[0].commit.summary, "fix logout redirect");
        assert_eq!(p2.commits[0].commit.summary, "Fix login bug");
        assert!(p2.next.is_none());
        assert!(search_history(&repos, 10, None, "nomatch")
            .commits
            .is_empty());
    }

    #[test]
    fn merged_history_survives_an_unreadable_or_empty_repo() {
        let good = init_empty_repo();
        commit_at(good.path(), "good", 1000);
        let empty = init_empty_repo();
        let missing = tempdir().unwrap(); // exists but is not a git repo
        let repos = [
            repo_of(1, "good", &good),
            repo_of(2, "empty", &empty),
            repo_of(3, "missing", &missing),
        ];

        let page = merged_history(&repos, 10, None);
        assert_eq!(summaries(&page), vec![("good", "good")]);
        assert_eq!(page.unreadable, vec![RepoId(3)]);
    }

    #[test]
    fn recent_commits_reads_head_history() {
        let dir = init_repo_with_commit("initial commit");
        let commits = recent_commits(dir.path(), 10).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "initial commit");
    }

    #[test]
    fn suggest_commits_matches_task_id_fragment() {
        let a = CommitInfo {
            hash: "a".into(),
            patch_id: "pa".into(),
            summary: "fix bug #42".into(),
            author: "x".into(),
            time: 0,
        };
        let b = CommitInfo {
            hash: "b".into(),
            patch_id: "pb".into(),
            summary: "unrelated".into(),
            author: "x".into(),
            time: 0,
        };
        let matches = suggest_commits_for_task(&[a.clone(), b], TaskId(42), None);
        assert_eq!(matches, vec![a]);
    }

    fn commit_info(summary: &str) -> CommitInfo {
        CommitInfo {
            hash: summary.into(),
            patch_id: String::new(),
            summary: summary.into(),
            author: "x".into(),
            time: 0,
        }
    }

    #[test]
    fn task_reference_matches_whole_tokens_only() {
        let hits =
            |s: &str| !suggest_commits_for_task(&[commit_info(s)], TaskId(4), None).is_empty();
        assert!(hits("fix #4"));
        assert!(hits("fix (#4)"));
        assert!(hits("#4: thing"));
        assert!(hits("[#4] thing"));
        assert!(hits("#4"));
        assert!(!hits("fix #42"));
        assert!(!hits("fix #4a"));
        assert!(!hits("fix #4_x"));
        assert!(!hits("no reference"));
    }

    #[test]
    fn branch_name_matches_case_insensitively_on_boundaries() {
        let hits = |s: &str| {
            !suggest_commits_for_task(&[commit_info(s)], TaskId(1), Some("feature/Login"))
                .is_empty()
        };
        assert!(hits("merge feature/login into main"));
        assert!(hits("wip (FEATURE/LOGIN)"));
        assert!(!hits("merge feature/login-v2"));
        assert!(!hits("prefeature/login"));
        // A blank branch must not match everything.
        let blank = suggest_commits_for_task(&[commit_info("anything")], TaskId(1), Some("  "));
        assert!(blank.is_empty());
    }

    #[test]
    fn non_ascii_summaries_do_not_panic() {
        for text in ["İstanbul #4 日本語", "ÀÉ#4é", "日本語 feat/ÜBER ✓", "#"] {
            let _ = suggest_commits_for_task(&[commit_info(text)], TaskId(4), Some("feat/über"));
        }
        let id = suggest_commits_for_task(&[commit_info("日本語 #4 完了")], TaskId(4), None);
        assert_eq!(id.len(), 1);
        let br = suggest_commits_for_task(
            &[commit_info("日本語 feat/ÜBER ✓")],
            TaskId(4),
            Some("feat/über"),
        );
        assert_eq!(br.len(), 1);
    }

    #[test]
    fn suggested_for_task_filters_orders_and_limits() {
        let a = init_empty_repo();
        let b = init_empty_repo();
        commit_at(a.path(), "a old #4", 100);
        commit_at(b.path(), "b mid #42 not it", 200);
        commit_at(b.path(), "b fix (#4)", 300);
        commit_at(a.path(), "a work on feat/x", 400);
        commit_at(a.path(), "a unrelated", 500);
        let repos = [repo_of(1, "a", &a), repo_of(2, "b", &b)];
        let none = std::collections::HashSet::new();

        let got = suggested_for_task(&repos, TaskId(4), Some("feat/x"), &none, 50, 10);
        let titles: Vec<_> = got.iter().map(|c| c.commit.summary.as_str()).collect();
        assert_eq!(titles, vec!["a work on feat/x", "b fix (#4)", "a old #4"]);
        assert_eq!(got[1].repo_name, "b");

        // Without a branch only the id matches.
        let ids = suggested_for_task(&repos, TaskId(4), None, &none, 50, 10);
        assert_eq!(ids.len(), 2);

        // max_results keeps the newest.
        let capped = suggested_for_task(&repos, TaskId(4), Some("feat/x"), &none, 50, 2);
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].commit.summary, "a work on feat/x");

        // Already linked commits are skipped.
        let linked: std::collections::HashSet<_> =
            [(got[1].repo_id, got[1].commit.hash.clone())].into();
        let rest = suggested_for_task(&repos, TaskId(4), Some("feat/x"), &linked, 50, 10);
        assert_eq!(rest.len(), 2);
        assert!(rest.iter().all(|c| c.commit.summary != "b fix (#4)"));

        // Identity is (repo, hash): the same hash under another repo id stays.
        let other_repo: std::collections::HashSet<_> =
            [(RepoId(99), got[1].commit.hash.clone())].into();
        let all = suggested_for_task(&repos, TaskId(4), Some("feat/x"), &other_repo, 50, 10);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn suggested_for_task_respects_scan_limit() {
        let a = init_empty_repo();
        commit_at(a.path(), "old #4", 100);
        commit_at(a.path(), "newer", 200);
        commit_at(a.path(), "newest", 300);
        let repos = [repo_of(1, "a", &a)];
        let none = std::collections::HashSet::new();
        assert!(suggested_for_task(&repos, TaskId(4), None, &none, 2, 10).is_empty());
        assert_eq!(
            suggested_for_task(&repos, TaskId(4), None, &none, 3, 10).len(),
            1
        );
        assert!(suggested_for_task(&repos, TaskId(4), None, &none, 3, 0).is_empty());
    }

    /// Repo whose unborn HEAD is `name`, so tests don't depend on the
    /// machine's `init.defaultBranch`.
    fn init_repo_on(name: &str) -> tempfile::TempDir {
        let dir = init_empty_repo();
        git_in(
            dir.path(),
            &["symbolic-ref", "HEAD", &format!("refs/heads/{name}")],
            None,
        );
        dir
    }

    /// main with one base commit, plus `feat` with three commits; HEAD ends
    /// on `feat`.
    fn repo_with_feature() -> tempfile::TempDir {
        let dir = init_repo_on("main");
        commit_at(dir.path(), "base", 100);
        git_in(dir.path(), &["checkout", "-q", "-b", "feat"], None);
        commit_at(dir.path(), "f1", 200);
        commit_at(dir.path(), "f2", 300);
        commit_at(dir.path(), "f3", 400);
        dir
    }

    fn view_titles(v: &BranchView) -> Vec<&str> {
        v.commits.iter().map(|c| c.summary.as_str()).collect()
    }

    #[test]
    fn branch_view_lists_only_commits_missing_from_target() {
        let dir = repo_with_feature();
        let repo = repo_of(1, "r", &dir);
        let v = branch_only_commits(&repo, "feat", None, 50).unwrap();
        assert!(v.branch_found);
        assert_eq!(v.target.as_deref(), Some("main"));
        assert_eq!(view_titles(&v), vec!["f3", "f2", "f1"]);
        assert!(!v.truncated);

        let explicit = branch_only_commits(&repo, "feat", Some("main"), 50).unwrap();
        assert_eq!(explicit, v);
    }

    #[test]
    fn branch_view_is_empty_after_merge() {
        let dir = repo_with_feature();
        git_in(dir.path(), &["checkout", "-q", "main"], None);
        git_in(
            dir.path(),
            &["merge", "-q", "--no-ff", "feat", "-m", "merge feat"],
            Some(500),
        );
        let repo = repo_of(1, "r", &dir);
        let v = branch_only_commits(&repo, "feat", None, 50).unwrap();
        assert!(v.branch_found);
        assert!(v.commits.is_empty());
        assert!(!v.truncated);
    }

    #[test]
    fn branch_view_missing_branch_is_not_an_error() {
        let dir = repo_with_feature();
        let repo = repo_of(1, "r", &dir);
        let v = branch_only_commits(&repo, "nope", None, 50).unwrap();
        assert!(!v.branch_found);
        assert!(v.commits.is_empty());
        assert_eq!(v.target, None);

        let empty = init_empty_repo();
        let v = branch_only_commits(&repo_of(2, "e", &empty), "feat", None, 5).unwrap();
        assert!(!v.branch_found);
    }

    #[test]
    fn branch_view_falls_back_to_origin_branch() {
        let dir = repo_with_feature();
        git_in(
            dir.path(),
            &["update-ref", "refs/remotes/origin/feat", "HEAD"],
            None,
        );
        git_in(dir.path(), &["checkout", "-q", "main"], None);
        git_in(dir.path(), &["branch", "-q", "-D", "feat"], None);
        let repo = repo_of(1, "r", &dir);
        let v = branch_only_commits(&repo, "feat", None, 50).unwrap();
        assert!(v.branch_found);
        assert_eq!(view_titles(&v), vec!["f3", "f2", "f1"]);
    }

    #[test]
    fn branch_view_truncates_at_limit() {
        let dir = repo_with_feature();
        let repo = repo_of(1, "r", &dir);
        let v = branch_only_commits(&repo, "feat", None, 2).unwrap();
        assert_eq!(view_titles(&v), vec!["f3", "f2"]);
        assert!(v.truncated);
        let exact = branch_only_commits(&repo, "feat", None, 3).unwrap();
        assert_eq!(exact.commits.len(), 3);
        assert!(!exact.truncated);
    }

    #[test]
    fn default_target_prefers_origin_head_then_main_master_develop() {
        let main = init_repo_on("main");
        commit_at(main.path(), "c", 100);
        assert_eq!(default_target_branch(main.path()).as_deref(), Some("main"));

        let master = init_repo_on("master");
        commit_at(master.path(), "c", 100);
        assert_eq!(
            default_target_branch(master.path()).as_deref(),
            Some("master")
        );

        let dev = init_repo_on("develop");
        commit_at(dev.path(), "c", 100);
        assert_eq!(
            default_target_branch(dev.path()).as_deref(),
            Some("develop")
        );

        let odd = init_repo_on("trunk");
        commit_at(odd.path(), "c", 100);
        assert_eq!(default_target_branch(odd.path()), None);
        git_in(
            odd.path(),
            &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
            None,
        );
        git_in(
            odd.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
            None,
        );
        assert_eq!(
            default_target_branch(odd.path()).as_deref(),
            Some("origin/trunk")
        );

        // Remote-only main is found as origin/main.
        let remote_only = init_repo_on("work");
        commit_at(remote_only.path(), "c", 100);
        git_in(
            remote_only.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
            None,
        );
        assert_eq!(
            default_target_branch(remote_only.path()).as_deref(),
            Some("origin/main")
        );

        let plain = tempdir().unwrap();
        assert_eq!(default_target_branch(plain.path()), None);
    }

    #[test]
    fn branch_view_without_any_target_is_unfiltered() {
        let dir = init_repo_on("trunk");
        commit_at(dir.path(), "t1", 100);
        commit_at(dir.path(), "t2", 200);
        let repo = repo_of(1, "r", &dir);
        let v = branch_only_commits(&repo, "trunk", None, 50).unwrap();
        assert!(v.branch_found);
        assert_eq!(v.target, None);
        assert_eq!(view_titles(&v), vec!["t2", "t1"]);

        // An explicit target that doesn't exist degrades the same way.
        let v = branch_only_commits(&repo, "trunk", Some("ghost"), 50).unwrap();
        assert_eq!(v.target, None);
        assert_eq!(v.commits.len(), 2);
    }

    #[test]
    fn branch_view_rejects_hostile_names() {
        let dir = repo_with_feature();
        let repo = repo_of(1, "r", &dir);
        for bad in ["--all", "-x", "a b", "fe\nat", "feat\tx", ""] {
            let v = branch_only_commits(&repo, bad, None, 50).unwrap();
            assert!(!v.branch_found, "{bad:?} must be treated as not found");
            assert!(v.commits.is_empty());
        }
        // Hostile target names are ignored, not passed to git.
        for bad in ["--all", "a b"] {
            let v = branch_only_commits(&repo, "feat", Some(bad), 50).unwrap();
            assert!(v.branch_found);
            assert_eq!(v.target, None);
            assert_eq!(v.commits.len(), 4, "unfiltered: feat has base + 3");
        }
    }

    // ---- filtered_history / list_branches ----

    fn commit_by(dir: &std::path::Path, message: &str, time: i64, author: &str) {
        std::fs::write(dir.join(format!("{time}.txt")), message).unwrap();
        git_in(dir, &["add", "."], None);
        let author = format!("--author={author} <x@example.com>");
        git_in(dir, &["commit", "-q", "-m", message, &author], Some(time));
    }

    fn titles(page: &HistoryPage) -> Vec<&str> {
        page.commits
            .iter()
            .map(|c| c.commit.summary.as_str())
            .collect()
    }

    fn run(dir: &std::path::Path, args: &[&str]) {
        git_in(dir, args, None);
    }

    /// Repo with `base` on the default branch and `feat 1/2` on `feature`,
    /// leaving the default branch checked out.
    fn repo_with_branch() -> (tempfile::TempDir, String) {
        let r = init_empty_repo();
        commit_at(r.path(), "base", 100);
        let default = {
            let git = Repository::open(r.path()).unwrap();
            let head = git.head().unwrap();
            head.shorthand().unwrap().to_owned()
        };
        run(r.path(), &["checkout", "-q", "-b", "feature"]);
        commit_at(r.path(), "feat 1", 200);
        commit_at(r.path(), "feat 2", 300);
        run(r.path(), &["checkout", "-q", &default]);
        (r, default)
    }

    fn branch_filter(name: &str) -> HistoryFilter {
        HistoryFilter {
            branch: Some(name.to_owned()),
            ..HistoryFilter::default()
        }
    }

    #[test]
    fn default_filter_matches_merged_history() {
        let (r, _) = repo_with_branch();
        let repos = [repo_of(1, "a", &r)];
        assert_eq!(
            filtered_history(&repos, 10, None, &HistoryFilter::default()),
            merged_history(&repos, 10, None)
        );
    }

    #[test]
    fn branch_filter_reads_that_branch_and_reports_repos_without_it() {
        let (a, _) = repo_with_branch();
        let b = init_empty_repo();
        commit_at(b.path(), "b only", 250);
        let empty = init_empty_repo();
        let repos = [
            repo_of(1, "a", &a),
            repo_of(2, "b", &b),
            repo_of(3, "empty", &empty),
        ];

        let page = filtered_history(&repos, 10, None, &branch_filter("feature"));
        assert_eq!(titles(&page), vec!["feat 2", "feat 1", "base"]);
        assert_eq!(page.missing_branch, vec![RepoId(2), RepoId(3)]);
        assert!(page.unreadable.is_empty());

        // HEAD of `a` does not have the feature commits.
        let head = filtered_history(&repos, 10, None, &HistoryFilter::default());
        assert!(!titles(&head).contains(&"feat 1"));
        assert!(head.missing_branch.is_empty());
    }

    #[test]
    fn branch_filter_falls_back_to_origin_and_prefers_local() {
        let (r, default) = repo_with_branch();
        let repos = [repo_of(1, "a", &r)];
        // Remote-only branch pointing at feature's tip, no local twin.
        run(
            r.path(),
            &["update-ref", "refs/remotes/origin/only-remote", "feature"],
        );
        let page = filtered_history(&repos, 10, None, &branch_filter("only-remote"));
        assert_eq!(titles(&page), vec!["feat 2", "feat 1", "base"]);

        // A stale origin/<default> must not shadow the local branch.
        run(
            r.path(),
            &[
                "update-ref",
                &format!("refs/remotes/origin/{default}"),
                "feature",
            ],
        );
        let page = filtered_history(&repos, 10, None, &branch_filter(&default));
        assert_eq!(titles(&page), vec!["base"]);
    }

    #[test]
    fn hostile_branch_names_match_nothing_and_do_not_error() {
        let (r, _) = repo_with_branch();
        let repos = [repo_of(1, "a", &r)];
        for bad in [
            "--all",
            "-x",
            "feature --all",
            "fea\nture",
            "..",
            "feature^{commit}",
            "HEAD~1 ",
            "origin/../feature",
        ] {
            let page = filtered_history(&repos, 10, None, &branch_filter(bad));
            assert!(page.commits.is_empty(), "{bad:?} must not match");
            assert!(page.unreadable.is_empty(), "{bad:?} is not an error");
            // Blank (whitespace-only) means "no branch filter".
        }
        let blank = filtered_history(&repos, 10, None, &branch_filter("  "));
        assert_eq!(titles(&blank), vec!["base"]);
    }

    #[test]
    fn date_range_is_inclusive_and_open_ended() {
        let r = init_empty_repo();
        for (m, t) in [("c1", 1000), ("c2", 2000), ("c3", 3000), ("c4", 4000)] {
            commit_at(r.path(), m, t);
        }
        let repos = [repo_of(1, "a", &r)];
        let range = |since, until| {
            let f = HistoryFilter {
                since,
                until,
                ..HistoryFilter::default()
            };
            let page = filtered_history(&repos, 10, None, &f);
            page.commits
                .iter()
                .map(|c| c.commit.summary.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(range(Some(2000), Some(3000)), vec!["c3", "c2"]);
        assert_eq!(range(Some(2001), Some(2999)), Vec::<String>::new());
        assert_eq!(range(Some(3000), None), vec!["c4", "c3"]);
        assert_eq!(range(None, Some(2000)), vec!["c2", "c1"]);
        assert_eq!(range(Some(4001), None), Vec::<String>::new());
        assert_eq!(range(None, Some(999)), Vec::<String>::new());
        assert_eq!(range(Some(3000), Some(2000)), Vec::<String>::new());
    }

    #[test]
    fn author_filter_is_case_insensitive_substring_on_name() {
        let r = init_empty_repo();
        commit_by(r.path(), "one", 100, "Alice Smith");
        commit_by(r.path(), "two", 200, "Bob Jones");
        commit_by(r.path(), "three", 300, "alice cooper");
        let repos = [repo_of(1, "a", &r)];
        let by = |a: &str| {
            let f = HistoryFilter {
                author: Some(a.to_owned()),
                ..HistoryFilter::default()
            };
            filtered_history(&repos, 10, None, &f)
        };
        assert_eq!(titles(&by("ALICE")), vec!["three", "one"]);
        assert_eq!(titles(&by("jones")), vec!["two"]);
        assert!(by("nobody").commits.is_empty());
        assert_eq!(by("  ").commits.len(), 3, "blank author is no filter");
        // Email is not part of the author-name match.
        assert!(by("example.com").commits.is_empty());
    }

    #[test]
    fn filters_combine_with_text_search_and_paging() {
        let a = init_empty_repo();
        let b = init_empty_repo();
        for i in 0..6 {
            commit_by(a.path(), &format!("fix a{i}"), 1000 + i * 10, "Alice");
            commit_by(b.path(), &format!("fix b{i}"), 1005 + i * 10, "Alice");
            commit_by(a.path(), &format!("feat a{i}"), 1002 + i * 10, "Bob");
        }
        let repos = [repo_of(1, "a", &a), repo_of(2, "b", &b)];
        let filter = HistoryFilter {
            text: "FIX".into(),
            author: Some("alice".into()),
            since: Some(1010),
            until: Some(1044),
            ..HistoryFilter::default()
        };
        let all = filtered_history(&repos, 100, None, &filter);
        // Alice fix commits, 1010 <= t <= 1044.
        let expected: Vec<&str> = vec![
            "fix a4", "fix b3", "fix a3", "fix b2", "fix a2", "fix b1", "fix a1",
        ];
        assert_eq!(titles(&all), expected);
        assert!(all.next.is_none());

        // Page by 4 then 2, exactly at the boundary: no repeats, no gaps.
        let p1 = filtered_history(&repos, 4, None, &filter);
        assert_eq!(titles(&p1), expected[..4].to_vec());
        let cursor = p1.next.clone().expect("more pages");
        let p2 = filtered_history(&repos, 4, Some(&cursor), &filter);
        assert_eq!(titles(&p2), expected[4..].to_vec());
        assert!(p2.next.is_none());

        // A page that ends exactly at the limit must not claim more.
        let exact = filtered_history(&repos, 7, None, &filter);
        assert_eq!(exact.commits.len(), 7);
        assert!(exact.next.is_none());
    }

    #[test]
    fn branch_filter_pages_across_repos_newest_first() {
        let (a, _) = repo_with_branch();
        let (b, _) = repo_with_branch();
        // Interleave: push b's feature commits a little later in time.
        run(b.path(), &["checkout", "-q", "feature"]);
        commit_at(b.path(), "b feat 3", 350);
        let repos = [repo_of(1, "a", &a), repo_of(2, "b", &b)];
        let filter = branch_filter("feature");
        let p1 = filtered_history(&repos, 3, None, &filter);
        assert_eq!(titles(&p1), vec!["b feat 3", "feat 2", "feat 2"]);
        let p2 = filtered_history(&repos, 3, p1.next.as_ref(), &filter);
        assert_eq!(titles(&p2), vec!["feat 1", "feat 1", "base"]);
        let p3 = filtered_history(&repos, 3, p2.next.as_ref(), &filter);
        assert_eq!(titles(&p3), vec!["base"]);
        assert!(p3.next.is_none());
    }

    #[test]
    fn since_stops_the_walk_early_but_keeps_newer_commits() {
        let r = init_empty_repo();
        for i in 0..50 {
            commit_at(r.path(), &format!("c{i}"), 1000 + i);
        }
        let repos = [repo_of(1, "a", &r)];
        let f = HistoryFilter {
            since: Some(1045),
            ..HistoryFilter::default()
        };
        let page = filtered_history(&repos, 100, None, &f);
        assert_eq!(page.commits.len(), 5);
        assert_eq!(page.commits[0].commit.summary, "c49");
    }

    #[test]
    fn list_branches_orders_current_then_local_then_remote_only() {
        let (r, default) = repo_with_branch();
        run(r.path(), &["branch", "alpha"]);
        run(r.path(), &["branch", "zeta"]);
        run(
            r.path(),
            &["update-ref", "refs/remotes/origin/beta", "feature"],
        );
        run(
            r.path(),
            &["update-ref", "refs/remotes/origin/alpha", "feature"],
        );
        run(
            r.path(),
            &["update-ref", "refs/remotes/origin/HEAD", "feature"],
        );
        let repo = repo_of(1, "a", &r);

        let branches = list_branches(&repo).unwrap();
        let mut locals = vec![
            "alpha".to_string(),
            "feature".to_string(),
            "zeta".to_string(),
        ];
        locals.retain(|b| *b != default);
        let mut expected = vec![default.clone()];
        expected.extend(locals);
        expected.sort_by_key(|b| *b != default); // stable: default first, rest alphabetical
        expected.push("beta".to_string());
        assert_eq!(branches, expected);
        assert!(!branches.iter().any(|b| b == "HEAD" || b.contains('/')));

        // Not a repo: an error, not a panic.
        let plain = tempdir().unwrap();
        assert!(list_branches(&repo_of(2, "x", &plain)).is_err());
    }

    #[test]
    fn project_branches_unions_repos_with_checked_out_first() {
        let (a, default) = repo_with_branch();
        let b = init_empty_repo();
        commit_at(b.path(), "b", 100);
        run(b.path(), &["checkout", "-q", "-b", "hotfix"]);
        let plain = tempdir().unwrap();
        let repos = [
            repo_of(1, "a", &a),
            repo_of(2, "b", &b),
            repo_of(3, "x", &plain),
        ];
        let all = project_branches(&repos);
        // Both checked-out branches lead, alphabetically; then the rest.
        let mut head: Vec<String> = vec![default.clone(), "hotfix".to_string()];
        head.sort();
        assert_eq!(all[..2].to_vec(), head);
        assert!(all.contains(&"feature".to_string()));
        let mut seen = std::collections::HashSet::new();
        assert!(all.iter().all(|b| seen.insert(b.clone())), "deduplicated");
    }
}
