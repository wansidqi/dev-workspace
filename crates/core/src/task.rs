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
    #[error("a tag must not be empty")]
    EmptyTag,
    #[error("a tag can be at most 32 characters")]
    TagTooLong,
    #[error("a tag cannot contain commas or control characters")]
    InvalidTagChar,
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

    /// Adds a tag (normalized, see [`normalize_tag`]). Tags are unique
    /// ignoring case and kept sorted that way. Returns `false` if the task
    /// already had it.
    pub fn add_tag(&mut self, raw: &str, now: i64) -> Result<bool, TaskError> {
        let tag = normalize_tag(raw)?;
        if self.has_tag(&tag) {
            return Ok(false);
        }
        self.tags.push(tag);
        self.tags.sort_by_key(|t| t.to_lowercase());
        self.updated_at = now;
        Ok(true)
    }

    /// Removes a tag, ignoring case. Returns whether it was present.
    pub fn remove_tag(&mut self, raw: &str, now: i64) -> bool {
        let wanted = raw.trim().to_lowercase();
        let before = self.tags.len();
        self.tags.retain(|t| t.to_lowercase() != wanted);
        let removed = self.tags.len() != before;
        if removed {
            self.updated_at = now;
        }
        removed
    }

    pub fn has_tag(&self, raw: &str) -> bool {
        let wanted = raw.trim().to_lowercase();
        self.tags.iter().any(|t| t.to_lowercase() == wanted)
    }
}

/// Longest tag, in characters. Tags are labels, not sentences.
pub const MAX_TAG_CHARS: usize = 32;

/// Validates and cleans a user-typed tag: surrounding whitespace is dropped
/// and inner whitespace runs collapse to one space. Rejects empty tags,
/// tags over [`MAX_TAG_CHARS`], and commas or control characters (commas
/// would make a comma-separated tag list ambiguous). Case is preserved —
/// uniqueness ignores it, display keeps what the user wrote.
pub fn normalize_tag(raw: &str) -> Result<String, TaskError> {
    if raw
        .chars()
        .any(|c| c == ',' || c.is_control() && !c.is_whitespace())
    {
        return Err(TaskError::InvalidTagChar);
    }
    let tag = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if tag.is_empty() {
        return Err(TaskError::EmptyTag);
    }
    if tag.chars().count() > MAX_TAG_CHARS {
        return Err(TaskError::TagTooLong);
    }
    Ok(tag)
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
    fn tags_are_normalized_unique_ignoring_case_and_sorted() {
        let mut t = Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap();
        assert_eq!(t.add_tag("  Backend  ", 5), Ok(true));
        assert_eq!(t.add_tag("api   v2", 6), Ok(true));
        assert_eq!(t.add_tag("BACKEND", 7), Ok(false), "same tag, other case");
        assert_eq!(t.tags, vec!["api v2".to_string(), "Backend".to_string()]);
        assert_eq!(t.updated_at, 6, "a rejected duplicate changes nothing");
        assert!(t.has_tag("backend"));
    }

    #[test]
    fn invalid_tags_are_rejected() {
        let mut t = Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap();
        assert_eq!(t.add_tag("", 0), Err(TaskError::EmptyTag));
        assert_eq!(t.add_tag("   ", 0), Err(TaskError::EmptyTag));
        assert_eq!(t.add_tag("a,b", 0), Err(TaskError::InvalidTagChar));
        assert_eq!(t.add_tag("a\u{0}b", 0), Err(TaskError::InvalidTagChar));
        let long = "x".repeat(MAX_TAG_CHARS + 1);
        assert_eq!(t.add_tag(&long, 0), Err(TaskError::TagTooLong));
        assert!(t.add_tag(&"x".repeat(MAX_TAG_CHARS), 0).is_ok());
        // Tabs and newlines are whitespace: collapsed, not rejected.
        assert_eq!(normalize_tag("a\tb\nc").unwrap(), "a b c");
        assert!(t.tags.len() == 1);
    }

    #[test]
    fn remove_tag_ignores_case_and_reports_presence() {
        let mut t = Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap();
        t.add_tag("Bug", 1).unwrap();
        assert!(t.remove_tag(" bug ", 2));
        assert!(!t.remove_tag("bug", 3));
        assert!(t.tags.is_empty());
        assert_eq!(t.updated_at, 2);
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
