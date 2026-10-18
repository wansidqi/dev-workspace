use std::collections::HashMap;

use devspace_core::{normalize_tag, ProjectId, Task, TaskId, TaskPriority, TaskStatus};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

pub struct TaskRepo;

impl TaskRepo {
    pub fn insert(conn: &Connection, task: &Task) -> Result<(), StorageError> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO tasks (id, project_id, parent_id, title, status, priority, due_at, branch, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id.0,
                task.project_id.0,
                task.parent_id.map(|id| id.0),
                task.title,
                status_to_str(task.status),
                priority_to_str(task.priority),
                task.due_at,
                task.branch,
                task.created_at,
                task.updated_at,
            ],
        )?;
        // A task built in memory with tags keeps them (same transaction, so
        // a bad tag leaves no half-saved task).
        for tag in &task.tags {
            Self::add_tag_in(&tx, task.id, tag)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Adds a tag to a task. The tag is normalized by `devspace-core`
    /// (trimmed, length and character rules) and is unique per task ignoring
    /// case. Returns `false` if the task already had it.
    pub fn add_tag(conn: &Connection, task: TaskId, tag: &str) -> Result<bool, StorageError> {
        Self::add_tag_in(conn, task, tag)
    }

    fn add_tag_in(conn: &Connection, task: TaskId, tag: &str) -> Result<bool, StorageError> {
        let tag = normalize_tag(tag).map_err(|e| StorageError::Domain(e.into()))?;
        let added = conn.execute(
            "INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
            params![task.0, tag],
        )?;
        Ok(added > 0)
    }

    /// Removes a tag from a task, ignoring case. Returns whether it existed.
    pub fn remove_tag(conn: &Connection, task: TaskId, tag: &str) -> Result<bool, StorageError> {
        // The column is COLLATE NOCASE, so this matches any capitalisation.
        let removed = conn.execute(
            "DELETE FROM task_tags WHERE task_id = ?1 AND tag = ?2",
            params![task.0, tag.trim()],
        )?;
        Ok(removed > 0)
    }

    fn tags_for_task(conn: &Connection, task: TaskId) -> Result<Vec<String>, StorageError> {
        let mut stmt = conn
            .prepare("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag COLLATE NOCASE")?;
        let rows = stmt.query_map(params![task.0], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn update(conn: &Connection, task: &Task) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE tasks SET parent_id = ?2, title = ?3, status = ?4, priority = ?5,
                due_at = ?6, branch = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                task.id.0,
                task.parent_id.map(|id| id.0),
                task.title,
                status_to_str(task.status),
                priority_to_str(task.priority),
                task.due_at,
                task.branch,
                task.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, id: TaskId) -> Result<Option<Task>, StorageError> {
        let task = conn
            .query_row(
                "SELECT id, project_id, parent_id, title, status, priority, due_at, branch, created_at, updated_at
                 FROM tasks WHERE id = ?1",
                params![id.0],
                Self::from_row,
            )
            .optional()?;
        match task {
            Some(mut task) => {
                task.tags = Self::tags_for_task(conn, id)?;
                Ok(Some(task))
            }
            None => Ok(None),
        }
    }

    pub fn list_for_project(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<Vec<Task>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, parent_id, title, status, priority, due_at, branch, created_at, updated_at
             FROM tasks WHERE project_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![project_id.0], Self::from_row)?;
        let mut tasks = rows.collect::<Result<Vec<_>, _>>()?;

        // One query for every tag in the project rather than one per task.
        let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT g.task_id, g.tag FROM task_tags g
             JOIN tasks t ON t.id = g.task_id
             WHERE t.project_id = ?1
             ORDER BY g.tag COLLATE NOCASE",
        )?;
        let pairs = stmt.query_map(params![project_id.0], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for pair in pairs {
            let (task_id, tag) = pair?;
            tags.entry(task_id).or_default().push(tag);
        }
        for task in &mut tasks {
            task.tags = tags.remove(&task.id.0).unwrap_or_default();
        }
        Ok(tasks)
    }

    pub fn delete(conn: &Connection, id: TaskId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Task> {
        let status: String = row.get(4)?;
        let priority: String = row.get(5)?;
        Ok(Task {
            id: TaskId(row.get(0)?),
            project_id: ProjectId(row.get(1)?),
            parent_id: row.get::<_, Option<i64>>(2)?.map(TaskId),
            title: row.get(3)?,
            status: str_to_status(&status),
            priority: str_to_priority(&priority),
            due_at: row.get(6)?,
            branch: row.get(7)?,
            tags: Vec::new(),
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    }
}

fn status_to_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Done => "done",
    }
}

fn str_to_status(s: &str) -> TaskStatus {
    match s {
        "in_progress" => TaskStatus::InProgress,
        "blocked" => TaskStatus::Blocked,
        "done" => TaskStatus::Done,
        _ => TaskStatus::Todo,
    }
}

fn priority_to_str(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "medium",
        TaskPriority::High => "high",
        TaskPriority::Urgent => "urgent",
    }
}

fn str_to_priority(s: &str) -> TaskPriority {
    match s {
        "low" => TaskPriority::Low,
        "high" => TaskPriority::High,
        "urgent" => TaskPriority::Urgent,
        _ => TaskPriority::Medium,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrations::run_migrations, open_in_memory, projects::ProjectRepo};
    use devspace_core::Project;

    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        let project = Project::new(ProjectId(1), "P", 0).unwrap();
        ProjectRepo::insert(&conn, &project).unwrap();
        conn
    }

    #[test]
    fn insert_then_get_round_trips() {
        let conn = setup();
        let task = Task::new(TaskId(1), ProjectId(1), "Do thing", 100).unwrap();
        TaskRepo::insert(&conn, &task).unwrap();
        let fetched = TaskRepo::get(&conn, TaskId(1)).unwrap().unwrap();
        assert_eq!(fetched, task);
    }

    #[test]
    fn list_for_project_orders_by_created_at() {
        let conn = setup();
        let mut a = Task::new(TaskId(1), ProjectId(1), "a", 200).unwrap();
        a.status = TaskStatus::Done;
        let b = Task::new(TaskId(2), ProjectId(1), "b", 100).unwrap();
        TaskRepo::insert(&conn, &a).unwrap();
        TaskRepo::insert(&conn, &b).unwrap();

        let tasks = TaskRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(tasks[0].title, "b");
        assert_eq!(tasks[1].title, "a");
        assert_eq!(tasks[1].status, TaskStatus::Done);
    }

    #[test]
    fn tags_round_trip_through_insert_get_and_list_sorted_ignoring_case() {
        let conn = setup();
        let mut task = Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap();
        task.add_tag("zeta", 0).unwrap();
        task.add_tag("Alpha", 0).unwrap();
        TaskRepo::insert(&conn, &task).unwrap();
        let plain = Task::new(TaskId(2), ProjectId(1), "plain", 1).unwrap();
        TaskRepo::insert(&conn, &plain).unwrap();

        let fetched = TaskRepo::get(&conn, TaskId(1)).unwrap().unwrap();
        assert_eq!(fetched.tags, vec!["Alpha".to_string(), "zeta".to_string()]);
        assert_eq!(fetched, task, "full round trip including tags");

        let listed = TaskRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(
            listed[0].tags,
            vec!["Alpha".to_string(), "zeta".to_string()]
        );
        assert!(listed[1].tags.is_empty(), "untagged task has no tags");
    }

    #[test]
    fn add_tag_is_unique_ignoring_case_and_remove_ignores_case() {
        let conn = setup();
        TaskRepo::insert(&conn, &Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap()).unwrap();

        assert!(TaskRepo::add_tag(&conn, TaskId(1), "  Bug ").unwrap());
        assert!(
            !TaskRepo::add_tag(&conn, TaskId(1), "BUG").unwrap(),
            "duplicate"
        );
        let tags = TaskRepo::get(&conn, TaskId(1)).unwrap().unwrap().tags;
        assert_eq!(
            tags,
            vec!["Bug".to_string()],
            "first spelling kept, trimmed"
        );

        assert!(TaskRepo::remove_tag(&conn, TaskId(1), "bUg").unwrap());
        assert!(!TaskRepo::remove_tag(&conn, TaskId(1), "bug").unwrap());
        assert!(TaskRepo::get(&conn, TaskId(1))
            .unwrap()
            .unwrap()
            .tags
            .is_empty());
    }

    #[test]
    fn storage_enforces_the_tag_rules_not_just_the_ui() {
        let conn = setup();
        TaskRepo::insert(&conn, &Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap()).unwrap();
        for bad in ["", "   ", "a,b", "a\u{7}b", &"x".repeat(33)] {
            assert!(
                TaskRepo::add_tag(&conn, TaskId(1), bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        // And a task for a tag that does not exist is refused by the foreign key.
        assert!(TaskRepo::add_tag(&conn, TaskId(99), "ok").is_err());
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn deleting_a_task_deletes_its_tags_and_tags_stay_per_project() {
        let conn = setup();
        ProjectRepo::insert(&conn, &Project::new(ProjectId(2), "Q", 0).unwrap()).unwrap();
        TaskRepo::insert(&conn, &Task::new(TaskId(1), ProjectId(1), "a", 0).unwrap()).unwrap();
        TaskRepo::insert(&conn, &Task::new(TaskId(2), ProjectId(2), "b", 0).unwrap()).unwrap();
        TaskRepo::add_tag(&conn, TaskId(1), "shared").unwrap();
        TaskRepo::add_tag(&conn, TaskId(2), "shared").unwrap();

        let p1 = TaskRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].tags, vec!["shared".to_string()]);

        TaskRepo::delete(&conn, TaskId(1)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "only the other project's tag remains");
    }

    #[test]
    fn update_persists_status_change() {
        let conn = setup();
        let mut task = Task::new(TaskId(1), ProjectId(1), "t", 0).unwrap();
        TaskRepo::insert(&conn, &task).unwrap();

        task.set_status(TaskStatus::InProgress, 50);
        TaskRepo::update(&conn, &task).unwrap();

        let fetched = TaskRepo::get(&conn, TaskId(1)).unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::InProgress);
    }
}
