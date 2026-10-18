use crate::{CommitLinkId, ProjectId, RepoId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitLinkStatus {
    Linked,
    NeedsMigration,
    CherryPicked,
    Deployed,
    /// The commit could not be found at its stored hash, likely because of
    /// a rebase or squash upstream.
    Missing,
}

impl CommitLinkStatus {
    /// Stable text form stored in the `commit_links.status` column. Never
    /// rename these: they are persisted.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linked => "linked",
            Self::NeedsMigration => "needs_migration",
            Self::CherryPicked => "cherry_picked",
            Self::Deployed => "deployed",
            Self::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "linked" => Self::Linked,
            "needs_migration" => Self::NeedsMigration,
            "cherry_picked" => Self::CherryPicked,
            "deployed" => Self::Deployed,
            "missing" => Self::Missing,
            _ => return None,
        })
    }
}

/// A link from a task to a git commit. Only the repo, hash and patch id are
/// stored; message/author/date are read from git at display time so this
/// never goes stale (see CLAUDE.md storage rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitLink {
    pub id: CommitLinkId,
    pub task_id: TaskId,
    pub repo_id: RepoId,
    pub hash: String,
    pub patch_id: Option<String>,
    pub status: CommitLinkStatus,
    pub note: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommitLinkError {
    #[error("commit hash must not be empty")]
    EmptyHash,
    /// The task and the repo belong to different projects. Commits can only
    /// be mapped to tasks within the same project.
    #[error("task and repo belong to different projects")]
    ProjectMismatch,
    #[error("this commit is already linked to that task")]
    AlreadyLinked,
    #[error("task not found")]
    TaskNotFound,
    #[error("repo not found")]
    RepoNotFound,
}

/// The project-scoping rule (CLAUDE.md): a link is valid only if the task's
/// project equals the repo's project, whatever depth the task sits at in
/// the subtask tree. Kept pure so storage can call it and tests can pin it.
pub fn check_same_project(
    task_project: ProjectId,
    repo_project: ProjectId,
) -> Result<(), CommitLinkError> {
    if task_project == repo_project {
        Ok(())
    } else {
        Err(CommitLinkError::ProjectMismatch)
    }
}

impl CommitLink {
    pub fn new(
        id: CommitLinkId,
        task_id: TaskId,
        repo_id: RepoId,
        hash: impl Into<String>,
        now: i64,
    ) -> Result<Self, CommitLinkError> {
        let hash = hash.into();
        if hash.trim().is_empty() {
            return Err(CommitLinkError::EmptyHash);
        }
        Ok(Self {
            id,
            task_id,
            repo_id,
            hash,
            patch_id: None,
            status: CommitLinkStatus::Linked,
            note: None,
            created_at: now,
        })
    }

    pub fn mark_missing(&mut self) {
        self.status = CommitLinkStatus::Missing;
    }
}

/// Orders commit links for a cherry-pick export: oldest `created_at` first,
/// since that is the order tasks typically accumulated linked commits in.
pub fn ordered_for_cherry_pick(mut links: Vec<CommitLink>) -> Vec<CommitLink> {
    links.sort_by_key(|l| l.created_at);
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_hash() {
        assert_eq!(
            CommitLink::new(CommitLinkId(1), TaskId(1), RepoId(1), "", 0).unwrap_err(),
            CommitLinkError::EmptyHash
        );
    }

    #[test]
    fn same_project_is_accepted_and_cross_project_rejected() {
        assert_eq!(check_same_project(ProjectId(1), ProjectId(1)), Ok(()));
        assert_eq!(
            check_same_project(ProjectId(1), ProjectId(2)),
            Err(CommitLinkError::ProjectMismatch)
        );
    }

    #[test]
    fn status_text_round_trips() {
        for status in [
            CommitLinkStatus::Linked,
            CommitLinkStatus::NeedsMigration,
            CommitLinkStatus::CherryPicked,
            CommitLinkStatus::Deployed,
            CommitLinkStatus::Missing,
        ] {
            assert_eq!(CommitLinkStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(CommitLinkStatus::parse("bogus"), None);
    }

    #[test]
    fn ordered_for_cherry_pick_sorts_by_created_at() {
        let a = CommitLink::new(CommitLinkId(1), TaskId(1), RepoId(1), "a", 200).unwrap();
        let b = CommitLink::new(CommitLinkId(2), TaskId(1), RepoId(1), "b", 100).unwrap();
        let ordered = ordered_for_cherry_pick(vec![a, b]);
        assert_eq!(ordered[0].hash, "b");
        assert_eq!(ordered[1].hash, "a");
    }
}
