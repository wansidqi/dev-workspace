use devspace_core::{ProjectId, Task, TaskId, TaskPriority, TaskStatus};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

pub struct TaskRepo;

impl TaskRepo {
    pub fn insert(conn: &Connection, task: &Task) -> Result<(), StorageError> {
        conn.execute(
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
        Ok(())
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
        conn.query_row(
            "SELECT id, project_id, parent_id, title, status, priority, due_at, branch, created_at, updated_at
             FROM tasks WHERE id = ?1",
            params![id.0],
            Self::from_row,
        )
        .optional()
        .map_err(StorageError::from)
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
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
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
