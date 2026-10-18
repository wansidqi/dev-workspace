//! Thin wrapper around `git2` for commit history and patch-id matching.
//! The git CLI is only shelled out to for cherry-pick export text
//! (see CLAUDE.md); everything else here uses libgit2 directly.

use devspace_core::{Repo, RepoId, TaskId};
use git2::Repository;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
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
}

type SortKey = (std::cmp::Reverse<i64>, RepoId, String);

fn sort_key(c: &RepoCommit) -> SortKey {
    (
        std::cmp::Reverse(c.commit.time),
        c.repo_id,
        c.commit.hash.clone(),
    )
}

/// Reads up to `limit` commits per repo from `repo`'s HEAD that sort after
/// `after`. Newest-first by commit time (libgit2 time sorting; with clock
/// skew inside one repo the order is as git itself reports it).
fn repo_page(
    repo: &Repo,
    limit: usize,
    after: Option<&SortKey>,
    needle: &str,
) -> Result<Vec<RepoCommit>, GitError> {
    let git = Repository::open(&repo.path)?;
    // A freshly `git init`-ed repo has no commits yet: empty, not broken.
    if git.is_empty()? {
        return Ok(Vec::new());
    }
    let mut revwalk = git.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME)?;
    revwalk.push_head()?;

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
        if after.is_some_and(|key| sort_key(&entry) <= *key) {
            continue;
        }
        if !matches_search(&entry.commit, needle) {
            continue;
        }
        out.push(entry);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
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
    let needle = text.trim().to_lowercase();
    let after_key = after.map(|c| (std::cmp::Reverse(c.time), c.repo_id, c.hash.clone()));
    let mut page = HistoryPage::default();
    let mut all = Vec::new();
    for repo in repos {
        // One extra per repo so "this repo has more" is detectable: without
        // it a page that ends exactly at the limit would claim to be last.
        match repo_page(repo, limit + 1, after_key.as_ref(), &needle) {
            Ok(commits) => all.extend(commits),
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
fn is_safe_ref_name(name: &str) -> bool {
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
}
