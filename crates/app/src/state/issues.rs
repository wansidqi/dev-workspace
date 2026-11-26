//! GitHub issue links of `AppState`: which tasks are issues, what changed
//! locally and still has to be sent, how a task's body (its first note) is
//! found, and how a pull from GitHub is applied. A child module of `state`
//! so it can use the private fields. Nothing here touches the network.

use super::{now, AppState};
use devspace_core::{
    parent_changes, plan_pull, status_from_remote, tags_from_labels, IssueLink, NoteId, NoteKind,
    RemoteIssue, RepoId, Task, TaskId,
};
use devspace_storage::{IssueLinkRepo, TaskRepo};

/// What was edited on a linked task since it was last sent to GitHub.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Dirty {
    pub title: bool,
    pub status: bool,
    pub tags: bool,
    pub body: bool,
}

/// What applying a pull changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PullStats {
    pub created: usize,
    pub updated: usize,
    pub reparented: usize,
}

impl PullStats {
    pub fn changed(&self) -> bool {
        self.created + self.updated + self.reparented > 0
    }
}

/// Title of the note that holds a task's body when one is made for it.
pub const BODY_NOTE_TITLE: &str = "Description";

impl AppState {
    pub(super) fn reload_issue_links(&mut self) {
        self.issue_links = match self.selected_project {
            Some(project) => {
                IssueLinkRepo::list_for_project(&self.conn, project).unwrap_or_default()
            }
            None => Vec::new(),
        };
    }

    /// The issue a task is linked to, if any.
    pub fn issue_link(&self, task: TaskId) -> Option<&IssueLink> {
        self.issue_links.iter().find(|l| l.task_id == task)
    }

    /// Records that a linked task was edited here, so the sync can send it.
    /// Unlinked tasks stay purely local and are never marked.
    pub(super) fn mark_dirty(&mut self, task: TaskId, change: impl FnOnce(&mut Dirty)) {
        if self.issue_link(task).is_some() {
            change(self.dirty.entry(task).or_default());
        }
    }

    /// Whether anything is waiting to be sent.
    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Takes the pending edits (oldest id first, for a stable order).
    pub fn take_dirty(&mut self) -> Vec<(TaskId, Dirty)> {
        let mut taken: Vec<(TaskId, Dirty)> = self.dirty.drain().collect();
        taken.sort_by_key(|(id, _)| id.0);
        taken
    }

    /// Puts edits back after a send failed, merging with newer ones.
    pub fn restore_dirty(&mut self, task: TaskId, old: Dirty) {
        let entry = self.dirty.entry(task).or_default();
        entry.title |= old.title;
        entry.status |= old.status;
        entry.tags |= old.tags;
        entry.body |= old.body;
    }

    /// The note that is a task's body: its oldest note. Tasks made from
    /// notes keep their note, and an imported issue's body is stored the
    /// same way, so the task page and GitHub show the same text.
    pub fn body_note(&self, task: TaskId) -> Option<NoteId> {
        self.notes
            .iter()
            .filter(|n| n.task_id == Some(task))
            .map(|n| n.id)
            .min_by_key(|id| id.0)
    }

    /// The body note, made (empty) if the task has none yet.
    pub fn ensure_body_note(&mut self, task: TaskId) -> Option<NoteId> {
        match self.body_note(task) {
            Some(id) => Some(id),
            None => self.create_note_of_kind(Some(task), BODY_NOTE_TITLE, NoteKind::Note),
        }
    }

    /// A task's body text (empty when it has none).
    pub fn body_text(&self, task: TaskId) -> String {
        self.body_note(task)
            .and_then(|id| self.read_note_content(id))
            .unwrap_or_default()
    }

    /// Notes of a task other than its body (old data may have several).
    pub fn extra_notes(&self, task: TaskId) -> Vec<devspace_core::Note> {
        let body = self.body_note(task);
        self.notes
            .iter()
            .filter(|n| n.task_id == Some(task) && Some(n.id) != body)
            .cloned()
            .collect()
    }

    /// Links a task to an issue it was just published as.
    pub fn link_task(&mut self, link: IssueLink) -> Result<(), String> {
        IssueLinkRepo::set(&self.conn, &link).map_err(|e| e.to_string())?;
        self.reload_issue_links();
        Ok(())
    }

    /// Remembers the issue's `updated_at` after a push, so the next pull
    /// does not take our own change for a remote one.
    pub fn set_link_remote_updated(&mut self, task: TaskId, remote_updated: &str) {
        if IssueLinkRepo::set_remote_updated(&self.conn, task, remote_updated).is_ok() {
            if let Some(link) = self.issue_links.iter_mut().find(|l| l.task_id == task) {
                link.remote_updated = remote_updated.to_string();
            }
        }
    }

    /// Sets a task's status directly.
    #[cfg(test)]
    pub fn set_task_status(&mut self, id: TaskId, status: devspace_core::TaskStatus) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if task.status == status {
            return;
        }
        task.set_status(status, now());
        let _ = TaskRepo::update(&self.conn, task);
        self.mark_dirty(id, |d| d.status = true);
    }

    /// Applies the issues fetched from one repo of the selected project:
    /// open issues without a task become tasks (with their body as the
    /// task's note and labels as tags), linked tasks follow issues that
    /// changed on GitHub, and sub-issues become subtasks. Remote changes are
    /// written straight to the database, not marked dirty, so a pull never
    /// echoes back as a push.
    pub fn apply_issue_pull(&mut self, repo: RepoId, issues: &[RemoteIssue]) -> PullStats {
        let mut stats = PullStats::default();
        let Some(project) = self.selected_project else {
            return stats;
        };
        if !self.repos.iter().any(|r| r.id == repo) {
            return stats;
        }
        let links: Vec<IssueLink> = self
            .issue_links
            .iter()
            .filter(|l| l.repo_id == repo)
            .cloned()
            .collect();
        let plan = plan_pull(&links, issues);

        // New tasks first, so their ids exist for the parent pass.
        let mut bodies: Vec<(TaskId, String)> = Vec::new();
        for issue in &plan.creates {
            let id = TaskId(Self::next_id(
                &mut self.next_task_id,
                self.tasks.iter().map(|t| t.id.0).max().unwrap_or(0),
            ));
            let Ok(mut task) = Task::new(id, project, issue.title.trim(), now()) else {
                continue;
            };
            task.tags = tags_from_labels(&issue.labels);
            let link = IssueLink {
                task_id: id,
                repo_id: repo,
                number: issue.number,
                issue_id: issue.id,
                remote_updated: issue.updated_at.clone(),
            };
            if TaskRepo::insert(&self.conn, &task).is_err()
                || IssueLinkRepo::set(&self.conn, &link).is_err()
            {
                // Leave no half-imported task behind.
                let _ = TaskRepo::delete(&self.conn, id);
                continue;
            }
            self.tasks.push(task);
            if !issue.body.trim().is_empty() {
                bodies.push((id, issue.body.clone()));
            }
            stats.created += 1;
        }
        self.reload_issue_links();

        for (task_id, issue) in &plan.updates {
            if self.apply_remote_issue(*task_id, issue) {
                stats.updated += 1;
            }
        }
        for (task_id, body) in bodies {
            self.store_body(task_id, &body);
        }

        let links: Vec<IssueLink> = self
            .issue_links
            .iter()
            .filter(|l| l.repo_id == repo)
            .cloned()
            .collect();
        for (task_id, parent) in parent_changes(&self.tasks, &links, issues) {
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                task.parent_id = parent;
                task.updated_at = now();
                if TaskRepo::update(&self.conn, task).is_ok() {
                    stats.reparented += 1;
                }
            }
        }
        stats
    }

    /// Brings one linked task in line with its issue. Returns whether the
    /// task still exists.
    fn apply_remote_issue(&mut self, task_id: TaskId, issue: &RemoteIssue) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) else {
            return false;
        };
        if !issue.title.trim().is_empty() {
            task.title = issue.title.trim().to_string();
        }
        task.status = status_from_remote(task.status, issue.closed);
        task.updated_at = now();
        let _ = TaskRepo::update(&self.conn, task);

        // Tags mirror labels on a linked task.
        let wanted = tags_from_labels(&issue.labels);
        let current = task.tags.clone();
        for tag in &current {
            if !wanted.iter().any(|w| w.eq_ignore_ascii_case(tag)) {
                let _ = TaskRepo::remove_tag(&self.conn, task_id, tag);
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    task.remove_tag(tag, now());
                }
            }
        }
        for tag in &wanted {
            if let Ok(true) = TaskRepo::add_tag(&self.conn, task_id, tag) {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    let _ = task.add_tag(tag, now());
                }
            }
        }

        if self.body_text(task_id) != issue.body {
            self.store_body(task_id, &issue.body);
        }
        self.set_link_remote_updated(task_id, &issue.updated_at);
        true
    }

    /// Writes a remote body into the task's note without marking it dirty.
    /// An empty remote body leaves a task without a note alone.
    fn store_body(&mut self, task_id: TaskId, body: &str) {
        if body.is_empty() && self.body_note(task_id).is_none() {
            return;
        }
        if let Some(id) = self.ensure_body_note(task_id) {
            self.store_note_content(id, body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devspace_core::TaskStatus;
    use devspace_storage::{open_in_memory, run_migrations, WorkspacePaths};

    fn setup() -> (AppState, std::path::PathBuf, RepoId) {
        let dir = std::env::temp_dir().join(format!(
            "devspace-issues-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        let mut state = AppState::new(conn, WorkspacePaths::new(dir.join("data")));
        state.create_project("P");
        state.link_repo("/not/a/real/repo", "repo");
        let repo = state.repos[0].id;
        (state, dir, repo)
    }

    fn issue(number: u64, title: &str, body: &str, updated: &str) -> RemoteIssue {
        RemoteIssue {
            number,
            id: number * 1000,
            title: title.into(),
            body: body.into(),
            closed: false,
            labels: vec!["bug".into()],
            parent_number: None,
            updated_at: updated.into(),
        }
    }

    #[test]
    fn a_pull_imports_issues_with_body_tags_and_hierarchy() {
        let (mut state, dir, repo) = setup();
        let mut child = issue(2, "Child", "child body", "t1");
        child.parent_number = Some(1);
        let stats = state.apply_issue_pull(repo, &[issue(1, "Parent", "# hi", "t1"), child]);
        assert_eq!(stats.created, 2);
        assert_eq!(stats.reparented, 1);

        let parent = state
            .tasks
            .iter()
            .find(|t| t.title == "Parent")
            .unwrap()
            .clone();
        let child = state
            .tasks
            .iter()
            .find(|t| t.title == "Child")
            .unwrap()
            .clone();
        assert_eq!(child.parent_id, Some(parent.id));
        assert_eq!(parent.tags, vec!["bug".to_string()]);
        assert_eq!(state.issue_link(parent.id).unwrap().number, 1);
        assert_eq!(state.body_text(parent.id), "# hi");
        assert_eq!(state.body_text(child.id), "child body");
        assert!(!state.has_dirty(), "a pull is not a local edit");

        // Pulling the same issues again changes nothing.
        let again = state.apply_issue_pull(
            repo,
            &[issue(1, "Parent", "# hi", "t1"), {
                let mut c = issue(2, "Child", "child body", "t1");
                c.parent_number = Some(1);
                c
            }],
        );
        assert!(!again.changed());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_remote_change_updates_title_status_tags_and_body() {
        let (mut state, dir, repo) = setup();
        state.apply_issue_pull(repo, &[issue(1, "Old", "old body", "t1")]);
        let id = state.tasks[0].id;
        state.set_task_status(id, TaskStatus::InProgress);
        let _ = state.take_dirty();

        let mut changed = issue(1, "New title", "new body", "t2");
        changed.labels = vec!["feature".into()];
        let stats = state.apply_issue_pull(repo, &[changed.clone()]);
        assert_eq!(stats.updated, 1);
        let task = state.tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(task.title, "New title");
        assert_eq!(
            task.status,
            TaskStatus::InProgress,
            "open stays in progress"
        );
        assert_eq!(task.tags, vec!["feature".to_string()]);
        assert_eq!(state.body_text(id), "new body");
        assert_eq!(state.issue_link(id).unwrap().remote_updated, "t2");

        changed.closed = true;
        changed.updated_at = "t3".into();
        state.apply_issue_pull(repo, &[changed]);
        assert_eq!(
            state.tasks.iter().find(|t| t.id == id).unwrap().status,
            TaskStatus::Done
        );
        assert!(!state.has_dirty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_linked_tasks_are_marked_dirty_and_edits_merge() {
        let (mut state, dir, repo) = setup();
        state.create_task("local only");
        let local = state.tasks[0].id;
        state.rename_task(local, "renamed locally");
        state.set_task_status(local, TaskStatus::Done);
        assert!(!state.has_dirty(), "a task without an issue never syncs");

        state.apply_issue_pull(repo, &[issue(1, "Linked", "b", "t1")]);
        let linked = state.issue_links[0].task_id;
        state.rename_task(linked, "Linked 2");
        state.set_task_status(linked, TaskStatus::Done);
        let body = state.body_note(linked).unwrap();
        state.write_note_content(body, "edited");
        let taken = state.take_dirty();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].0, linked);
        assert_eq!(
            taken[0].1,
            Dirty {
                title: true,
                status: true,
                tags: false,
                body: true
            }
        );
        assert!(!state.has_dirty());
        state.restore_dirty(linked, taken[0].1);
        assert!(state.has_dirty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_oldest_note_is_the_body_and_the_rest_are_extras() {
        let (mut state, dir, _) = setup();
        state.create_task("T");
        let task = state.tasks[0].id;
        assert_eq!(state.body_note(task), None);
        let first = state.ensure_body_note(task).unwrap();
        let second = state.create_note_for(Some(task), "More").unwrap();
        assert_eq!(state.body_note(task), Some(first));
        assert_eq!(state.ensure_body_note(task), Some(first), "not made twice");
        assert_eq!(
            state
                .extra_notes(task)
                .iter()
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            vec![second]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_a_linked_task_drops_its_link() {
        let (mut state, dir, repo) = setup();
        state.apply_issue_pull(repo, &[issue(1, "A", "", "t1")]);
        let id = state.tasks[0].id;
        state.delete_task(id);
        assert!(state.issue_links.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
