//! Tasks and GitHub issues as one thing. A task may be linked to an issue;
//! GitHub is the source of truth for linked tasks. The functions here decide
//! what a pull has to change, with no network and no database, so the rules
//! are unit-tested on their own.

use std::collections::HashMap;

use crate::{descendants_of, normalize_tag, RepoId, Task, TaskId, TaskStatus};

/// A task that is also a GitHub issue. The link is optional: a task without
/// one is purely local and never leaves the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLink {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    /// The issue number shown as `#123`; unique within a repo.
    pub number: u64,
    /// GitHub's database id, which the sub-issues API needs instead of the
    /// number.
    pub issue_id: u64,
    /// The issue's `updated_at` when it was last pulled or pushed; a pull
    /// that sees the same value has nothing new to apply.
    pub remote_updated: String,
}

/// What a pull needs of an issue, free of the GitHub client's types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIssue {
    pub number: u64,
    pub id: u64,
    pub title: String,
    pub body: String,
    pub closed: bool,
    pub labels: Vec<String>,
    /// The issue this is a sub-issue of (same repo).
    pub parent_number: Option<u64>,
    pub updated_at: String,
}

/// Whether a task status closes its issue: GitHub only knows open and
/// closed, so only Done closes; Todo, In Progress and Blocked stay open.
pub fn status_closes_issue(status: TaskStatus) -> bool {
    status == TaskStatus::Done
}

/// The status a task takes when its issue changed remotely. The finer local
/// statuses (In Progress, Blocked) survive while the issue stays open.
pub fn status_from_remote(local: TaskStatus, closed: bool) -> TaskStatus {
    match (closed, local) {
        (true, _) => TaskStatus::Done,
        (false, TaskStatus::Done) => TaskStatus::Todo,
        (false, other) => other,
    }
}

/// The labels of an issue that can be tasks' tags: labels over the tag rules
/// (too long, with a comma) are skipped, and duplicates ignoring case go.
pub fn tags_from_labels(labels: &[String]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for label in labels {
        let Ok(tag) = normalize_tag(label) else {
            continue;
        };
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            tags.push(tag);
        }
    }
    tags
}

/// The label list to push when a task's tags changed: every tag, plus the
/// remote labels that cannot be tags and so would otherwise be wiped.
pub fn labels_to_push(remote_labels: &[String], tags: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = remote_labels
        .iter()
        .filter(|l| normalize_tag(l).is_err())
        .cloned()
        .collect();
    for tag in tags {
        // Keep GitHub's spelling when the label already exists.
        let spelled = remote_labels
            .iter()
            .find(|l| l.eq_ignore_ascii_case(tag))
            .unwrap_or(tag);
        labels.push(spelled.clone());
    }
    labels
}

/// The issues a pull must bring in or refresh.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PullPlan {
    /// Open issues with no task yet: they become tasks.
    pub creates: Vec<RemoteIssue>,
    /// Linked tasks whose issue changed since the last sync.
    pub updates: Vec<(TaskId, RemoteIssue)>,
}

/// Compares the issues fetched from one repo with that repo's links.
/// Closed issues without a task are not imported (history is not backlog);
/// a linked task is updated only when the issue's `updated_at` moved.
pub fn plan_pull(links: &[IssueLink], issues: &[RemoteIssue]) -> PullPlan {
    let by_number: HashMap<u64, &IssueLink> = links.iter().map(|l| (l.number, l)).collect();
    let mut plan = PullPlan::default();
    for issue in issues {
        match by_number.get(&issue.number) {
            None if !issue.closed => plan.creates.push(issue.clone()),
            None => {}
            Some(link) if link.remote_updated != issue.updated_at => {
                plan.updates.push((link.task_id, issue.clone()));
            }
            Some(_) => {}
        }
    }
    plan
}

/// The parent each linked task should have, going by GitHub's sub-issues:
/// `(task, new parent)` for every task whose parent differs. A parent that is
/// not imported leaves the task where it is; an issue with no parent on
/// GitHub leaves a local grouping parent alone unless that parent is itself an
/// issue (then the task was moved out on GitHub). Never makes a cycle.
pub fn parent_changes(
    tasks: &[Task],
    links: &[IssueLink],
    issues: &[RemoteIssue],
) -> Vec<(TaskId, Option<TaskId>)> {
    let task_of: HashMap<u64, TaskId> = links.iter().map(|l| (l.number, l.task_id)).collect();
    let linked: HashMap<TaskId, ()> = links.iter().map(|l| (l.task_id, ())).collect();
    let mut changes = Vec::new();
    for issue in issues {
        let Some(&task_id) = task_of.get(&issue.number) else {
            continue;
        };
        let Some(task) = tasks.iter().find(|t| t.id == task_id) else {
            continue;
        };
        match issue.parent_number {
            Some(parent_number) => {
                let Some(&parent) = task_of.get(&parent_number) else {
                    continue;
                };
                if task.parent_id == Some(parent) || parent == task_id {
                    continue;
                }
                if descendants_of(tasks, task_id).contains(&parent) {
                    continue;
                }
                changes.push((task_id, Some(parent)));
            }
            None => {
                if let Some(current) = task.parent_id {
                    if linked.contains_key(&current) {
                        changes.push((task_id, None));
                    }
                }
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectId;

    fn issue(number: u64, updated: &str) -> RemoteIssue {
        RemoteIssue {
            number,
            id: number * 1000,
            title: format!("issue {number}"),
            body: String::new(),
            closed: false,
            labels: Vec::new(),
            parent_number: None,
            updated_at: updated.into(),
        }
    }

    fn link(task: i64, number: u64, updated: &str) -> IssueLink {
        IssueLink {
            task_id: TaskId(task),
            repo_id: RepoId(1),
            number,
            issue_id: number * 1000,
            remote_updated: updated.into(),
        }
    }

    fn task(id: i64, parent: Option<i64>) -> Task {
        let mut t = Task::new(TaskId(id), ProjectId(1), "t", 0).unwrap();
        t.parent_id = parent.map(TaskId);
        t
    }

    #[test]
    fn only_done_closes_an_issue() {
        assert!(status_closes_issue(TaskStatus::Done));
        for open in [
            TaskStatus::Todo,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
        ] {
            assert!(!status_closes_issue(open));
        }
    }

    #[test]
    fn remote_state_keeps_fine_local_statuses_while_open() {
        use TaskStatus::*;
        assert_eq!(status_from_remote(InProgress, false), InProgress);
        assert_eq!(status_from_remote(Blocked, false), Blocked);
        assert_eq!(status_from_remote(Done, false), Todo, "reopened");
        assert_eq!(status_from_remote(InProgress, true), Done);
        assert_eq!(status_from_remote(Todo, true), Done);
    }

    #[test]
    fn labels_become_tags_when_they_fit_the_tag_rules() {
        let labels = vec![
            "bug".to_string(),
            "Bug".to_string(),
            "has,comma".to_string(),
            "x".repeat(40),
            "good first issue".to_string(),
        ];
        assert_eq!(
            tags_from_labels(&labels),
            vec!["bug".to_string(), "good first issue".to_string()]
        );
    }

    #[test]
    fn pushing_tags_never_wipes_labels_that_cannot_be_tags() {
        let remote = vec!["has,comma".to_string(), "Bug".to_string()];
        let pushed = labels_to_push(&remote, &["bug".to_string(), "new".to_string()]);
        assert_eq!(
            pushed,
            vec![
                "has,comma".to_string(),
                "Bug".to_string(),
                "new".to_string()
            ],
            "keeps GitHub's spelling and the unrepresentable label"
        );
        assert!(labels_to_push(&remote, &[]).contains(&"has,comma".to_string()));
    }

    #[test]
    fn a_pull_imports_open_issues_and_refreshes_only_changed_links() {
        let links = vec![link(1, 1, "t1"), link(2, 2, "t1")];
        let mut closed_unknown = issue(5, "t9");
        closed_unknown.closed = true;
        let issues = vec![
            issue(1, "t1"),
            issue(2, "t2"),
            issue(3, "t3"),
            closed_unknown,
        ];
        let plan = plan_pull(&links, &issues);
        assert_eq!(
            plan.creates.iter().map(|i| i.number).collect::<Vec<_>>(),
            [3]
        );
        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].0, TaskId(2));
    }

    #[test]
    fn closing_a_linked_issue_is_an_update_not_an_import() {
        let links = vec![link(1, 1, "t1")];
        let mut closed = issue(1, "t2");
        closed.closed = true;
        let plan = plan_pull(&links, &[closed]);
        assert!(plan.creates.is_empty());
        assert_eq!(plan.updates.len(), 1);
    }

    #[test]
    fn sub_issues_become_subtasks_and_unparenting_follows_github() {
        let tasks = vec![task(1, None), task(2, None), task(3, Some(1))];
        let links = vec![link(1, 10, "t"), link(2, 11, "t"), link(3, 12, "t")];
        let mut child = issue(11, "t");
        child.parent_number = Some(10);
        let mut moved_out = issue(12, "t");
        moved_out.parent_number = None;
        let changes = parent_changes(&tasks, &links, &[issue(10, "t"), child, moved_out]);
        assert_eq!(
            changes,
            vec![(TaskId(2), Some(TaskId(1))), (TaskId(3), None)]
        );
    }

    #[test]
    fn a_local_grouping_parent_survives_and_unknown_parents_are_ignored() {
        // Task 2 sits under task 1, which is only a local folder (no issue).
        let tasks = vec![task(1, None), task(2, Some(1)), task(3, None)];
        let links = vec![link(2, 11, "t"), link(3, 12, "t")];
        let mut orphan = issue(12, "t");
        orphan.parent_number = Some(99); // its parent was never imported
        assert!(parent_changes(&tasks, &links, &[issue(11, "t"), orphan]).is_empty());
    }

    #[test]
    fn a_parent_change_never_creates_a_cycle() {
        // GitHub says 1 is a child of 2, but locally 2 already hangs off 1.
        let tasks = vec![task(1, None), task(2, Some(1))];
        let links = vec![link(1, 10, "t"), link(2, 11, "t")];
        let mut one = issue(10, "t");
        one.parent_number = Some(11);
        let mut two = issue(11, "t");
        two.parent_number = Some(10);
        assert!(parent_changes(&tasks, &links, &[one, two]).is_empty());
    }
}
