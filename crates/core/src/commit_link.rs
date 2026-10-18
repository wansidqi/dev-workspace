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

/// One commit to export, as resolved against git by the caller. `core` has
/// no git access, so "could git find it" is encoded in `time`/`summary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CherryPickCommit {
    pub hash: String,
    /// First line of the commit message; `None` when git could not find it.
    pub summary: Option<String>,
    /// Commit time (unix seconds); `None` means git could not find the
    /// commit (rebased or squashed away, or the repo moved).
    pub time: Option<i64>,
}

/// The commits of one repo to export together, since a cherry-pick must run
/// inside the repo the commits belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CherryPickGroup {
    pub repo_name: String,
    pub repo_path: String,
    pub commits: Vec<CherryPickCommit>,
}

/// Collapses control characters (newlines included) to spaces so that
/// untrusted text can never start a new line in the generated script.
fn single_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Whether a stored hash is safe to place in a shell command: only hex
/// digits, so it cannot carry options, quotes or metacharacters.
fn is_safe_hash(hash: &str) -> bool {
    (7..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Builds shell text that cherry-picks each group's commits, oldest first.
///
/// Only comment lines and one `git cherry-pick` line per group are ever
/// emitted, and every untrusted string is either a comment (newlines
/// stripped) or a validated hex hash, so pasting the output into a terminal
/// cannot run anything unexpected. No `cd` is emitted because path quoting
/// differs per shell; the repo path is given as a comment instead.
pub fn cherry_pick_script(groups: &[CherryPickGroup]) -> String {
    let sections: Vec<String> = groups
        .iter()
        .filter(|g| !g.commits.is_empty())
        .map(cherry_pick_section)
        .collect();
    sections.join("\n\n")
}

fn cherry_pick_section(group: &CherryPickGroup) -> String {
    let mut lines = vec![
        format!("# {}", single_line(&group.repo_name)),
        format!("# run inside: {}", single_line(&group.repo_path)),
    ];

    let mut resolvable: Vec<(i64, &CherryPickCommit)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for commit in &group.commits {
        if !is_safe_hash(&commit.hash) {
            problems.push("# INVALID HASH SKIPPED".to_string());
        } else if let Some(time) = commit.time {
            resolvable.push((time, commit));
        } else {
            problems.push(format!("# MISSING (rebased or squashed?): {}", commit.hash));
        }
    }
    // Stable sort keeps input order for commits with equal timestamps.
    resolvable.sort_by_key(|(time, _)| *time);

    for (_, commit) in &resolvable {
        let short = &commit.hash[..7];
        match commit.summary.as_deref() {
            Some(summary) => lines.push(format!("# {short} {}", single_line(summary))),
            None => lines.push(format!("# {short}")),
        }
    }
    if !resolvable.is_empty() {
        let hashes: Vec<&str> = resolvable.iter().map(|(_, c)| c.hash.as_str()).collect();
        lines.push(format!("git cherry-pick {}", hashes.join(" ")));
    }
    lines.extend(problems);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const H2: &str = "2222222bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const H3: &str = "3333333ccccccccccccccccccccccccccccccccc";

    fn commit(hash: &str, summary: &str, time: i64) -> CherryPickCommit {
        CherryPickCommit {
            hash: hash.to_string(),
            summary: Some(summary.to_string()),
            time: Some(time),
        }
    }

    fn missing(hash: &str) -> CherryPickCommit {
        CherryPickCommit {
            hash: hash.to_string(),
            summary: None,
            time: None,
        }
    }

    fn group(name: &str, commits: Vec<CherryPickCommit>) -> CherryPickGroup {
        CherryPickGroup {
            repo_name: name.to_string(),
            repo_path: format!("/work/{name}"),
            commits,
        }
    }

    #[test]
    fn orders_oldest_first_with_stable_ties() {
        let g = group(
            "api",
            vec![
                commit(H1, "late", 300),
                commit(H2, "tie a", 100),
                commit(H3, "tie b", 100),
            ],
        );
        let out = cherry_pick_script(&[g]);
        assert_eq!(
            out,
            format!(
                "# api\n# run inside: /work/api\n# 2222222 tie a\n# 3333333 tie b\n# 1111111 late\ngit cherry-pick {H2} {H3} {H1}"
            )
        );
    }

    #[test]
    fn multiple_repos_are_separated_and_empty_groups_skipped() {
        let out = cherry_pick_script(&[
            group("a", vec![commit(H1, "one", 1)]),
            group("empty", vec![]),
            group("b", vec![commit(H2, "two", 2)]),
        ]);
        let sections: Vec<&str> = out.split("\n\n").collect();
        assert_eq!(sections.len(), 2);
        assert!(sections[0].starts_with("# a\n"));
        assert!(sections[1].starts_with("# b\n"));
        assert!(!out.contains("empty"));
    }

    #[test]
    fn missing_commit_is_excluded_and_listed_after_command() {
        let out = cherry_pick_script(&[group("api", vec![missing(H3), commit(H1, "ok", 5)])]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[lines.len() - 2], format!("git cherry-pick {H1}"));
        assert_eq!(
            lines[lines.len() - 1],
            format!("# MISSING (rebased or squashed?): {H3}")
        );
        assert_eq!(out.matches("git cherry-pick").count(), 1);
    }

    #[test]
    fn all_missing_group_has_no_cherry_pick_line() {
        let out = cherry_pick_script(&[group("api", vec![missing(H1), missing(H2)])]);
        assert!(!out.contains("git cherry-pick"));
        assert_eq!(out.matches("# MISSING").count(), 2);
        assert!(out.starts_with("# api\n# run inside: /work/api\n"));
    }

    #[test]
    fn invalid_hash_is_never_echoed() {
        let bad = "abc; rm -rf /";
        let too_short = "abc123";
        let non_hex = "zzzzzzzzzz";
        let out = cherry_pick_script(&[group(
            "api",
            vec![
                commit(bad, "x", 1),
                commit(too_short, "y", 2),
                commit(non_hex, "z", 3),
                commit(H1, "ok", 4),
            ],
        )]);
        assert_eq!(out.matches("# INVALID HASH SKIPPED").count(), 3);
        assert!(!out.contains("rm -rf"));
        assert!(!out.contains(too_short));
        assert!(!out.contains(non_hex));
        assert!(out.contains(&format!("git cherry-pick {H1}\n")));
    }

    #[test]
    fn hostile_text_cannot_start_a_command_line() {
        let evil = "x\nrm -rf ~\r\n; echo hi && $(whoami) `id`\u{2028}";
        let g = CherryPickGroup {
            repo_name: evil.to_string(),
            repo_path: evil.to_string(),
            commits: vec![commit(H1, evil, 1), commit(H2, evil, 2), missing(H3)],
        };
        let out = cherry_pick_script(&[g]);
        for line in out.lines() {
            assert!(
                line.starts_with("# ") || line == format!("git cherry-pick {H1} {H2}"),
                "unexpected line: {line:?}"
            );
        }
        assert_eq!(out.matches("git cherry-pick").count(), 1);
        assert!(!out.contains('\r'));
    }

    #[test]
    fn empty_input_gives_empty_string() {
        assert_eq!(cherry_pick_script(&[]), "");
        assert_eq!(cherry_pick_script(&[group("a", vec![])]), "");
    }

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
