//! Thin wrapper around `git2` for commit history and patch-id matching.
//! The git CLI is only shelled out to for cherry-pick export text
//! (see CLAUDE.md); everything else here uses libgit2 directly.

use devspace_core::TaskId;
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
