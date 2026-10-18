use crate::{ProjectId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub parent_id: Option<TaskId>,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub due_at: Option<i64>,
    pub branch: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaskError {
    #[error("task title must not be empty")]
    EmptyTitle,
    #[error("a task cannot be its own parent")]
    SelfParent,
}

impl Task {
    pub fn new(
        id: TaskId,
        project_id: ProjectId,
        title: impl Into<String>,
        now: i64,
    ) -> Result<Self, TaskError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TaskError::EmptyTitle);
        }
        Ok(Self {
            id,
            project_id,
            parent_id: None,
            title,
            status: TaskStatus::Todo,
            priority: TaskPriority::Medium,
            due_at: None,
            branch: None,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
        })
    }

    pub fn set_parent(&mut self, parent_id: Option<TaskId>, now: i64) -> Result<(), TaskError> {
        if parent_id == Some(self.id) {
            return Err(TaskError::SelfParent);
        }
        self.parent_id = parent_id;
        self.updated_at = now;
        Ok(())
    }

    pub fn set_status(&mut self, status: TaskStatus, now: i64) {
        self.status = status;
        self.updated_at = now;
    }
}

/// Given a flat list of tasks, returns the ids of every descendant of
/// `root`, walking the parent/child links. Used to cascade status changes
/// or block deletion of a task that still has subtasks.
pub fn descendants_of(tasks: &[Task], root: TaskId) -> Vec<TaskId> {
    let mut result = Vec::new();
    let mut frontier = vec![root];
    while let Some(current) = frontier.pop() {
        for t in tasks {
            if t.parent_id == Some(current) {
                result.push(t.id);
                frontier.push(t.id);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: i64, parent: Option<i64>) -> Task {
        let mut t = Task::new(TaskId(id), ProjectId(1), "t", 0).unwrap();
        t.parent_id = parent.map(TaskId);
        t
    }

    #[test]
    fn new_rejects_empty_title() {
        assert_eq!(
            Task::new(TaskId(1), ProjectId(1), "", 0).unwrap_err(),
            TaskError::EmptyTitle
        );
    }

    #[test]
    fn set_parent_rejects_self_parent() {
        let mut t = Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap();
        assert_eq!(
            t.set_parent(Some(TaskId(1)), 0).unwrap_err(),
            TaskError::SelfParent
        );
    }

    #[test]
    fn descendants_of_walks_nested_children() {
        let tasks = vec![
            task(1, None),
            task(2, Some(1)),
            task(3, Some(2)),
            task(4, None),
        ];
        let mut ids: Vec<i64> = descendants_of(&tasks, TaskId(1))
            .into_iter()
            .map(|id| id.0)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![2, 3]);
    }
}
