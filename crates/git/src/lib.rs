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

/// Returns the commits in `repo` whose message or author mentions a task
/// id or branch name fragment — used to surface "suggested commits" for a
/// task that hasn't had anything linked yet.
pub fn suggest_commits_for_task(
    commits: &[CommitInfo],
    task_id: TaskId,
    branch: Option<&str>,
) -> Vec<CommitInfo> {
    let id_fragment = format!("#{}", task_id.0);
    commits
        .iter()
        .filter(|c| {
            c.summary.contains(&id_fragment) || branch.is_some_and(|b| c.summary.contains(b))
        })
        .cloned()
        .collect()
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
}
