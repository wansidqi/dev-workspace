use devspace_core::{Attachment, AttachmentId, ProjectId, TaskId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

const COLUMNS: &str = "id, project_id, task_id, name, file_path, is_linked, created_at";

/// Persistence for files attached to a project, task or subtask. Copied
/// attachments store a path relative to the project's `attachments/`
/// directory; linked ones store the absolute path they point at.
pub struct AttachmentRepo;

impl AttachmentRepo {
    /// Inserts an attachment. If it targets a task, that task must belong to
    /// the attachment's project (a task may sit at any subtask depth).
    pub fn insert(conn: &Connection, attachment: &Attachment) -> Result<(), StorageError> {
        if let Some(task_id) = attachment.task_id {
            let task_project: Option<i64> = conn
                .query_row(
                    "SELECT project_id FROM tasks WHERE id = ?1",
                    params![task_id.0],
                    |row| row.get(0),
                )
                .optional()?;
            if task_project != Some(attachment.project_id.0) {
                return Err(StorageError::Domain(anyhow::anyhow!(
                    "attachment task does not belong to the attachment's project"
                )));
            }
        }
        conn.execute(
            "INSERT INTO attachments (id, project_id, task_id, name, file_path, is_linked, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                attachment.id.0,
                attachment.project_id.0,
                attachment.task_id.map(|t| t.0),
                attachment.name,
                attachment.file_path,
                attachment.is_linked,
                attachment.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, id: AttachmentId) -> Result<Option<Attachment>, StorageError> {
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM attachments WHERE id = ?1"),
            params![id.0],
            from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    /// Every attachment in a project (project-level and per-task), oldest
    /// first. Callers group by `task_id`.
    pub fn list_for_project(
        conn: &Connection,
        project: ProjectId,
    ) -> Result<Vec<Attachment>, StorageError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM attachments WHERE project_id = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![project.0], from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn list_for_task(conn: &Connection, task: TaskId) -> Result<Vec<Attachment>, StorageError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM attachments WHERE task_id = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![task.0], from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete(conn: &Connection, id: AttachmentId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM attachments WHERE id = ?1", params![id.0])?;
        Ok(())
    }
}

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: AttachmentId(row.get(0)?),
        project_id: ProjectId(row.get(1)?),
        task_id: row.get::<_, Option<i64>>(2)?.map(TaskId),
        name: row.get(3)?,
        file_path: row.get(4)?,
        is_linked: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migrations::run_migrations, open_in_memory, projects::ProjectRepo, tasks::TaskRepo,
    };
    use devspace_core::{Project, Task};

    /// Projects 1 and 2; task 1 in project 1, task 2 in project 2.
    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for p in [1, 2] {
            ProjectRepo::insert(&conn, &Project::new(ProjectId(p), "P", 0).unwrap()).unwrap();
        }
        TaskRepo::insert(&conn, &Task::new(TaskId(1), ProjectId(1), "t1", 0).unwrap()).unwrap();
        TaskRepo::insert(&conn, &Task::new(TaskId(2), ProjectId(2), "t2", 0).unwrap()).unwrap();
        conn
    }

    fn attachment(id: i64, project: i64, task: Option<i64>, linked: bool) -> Attachment {
        let path = if linked {
            "C:/docs/spec.pdf"
        } else {
            "1-spec.pdf"
        };
        let mut a = Attachment::new(
            AttachmentId(id),
            ProjectId(project),
            "spec.pdf",
            path,
            linked,
            id,
        )
        .unwrap();
        a.task_id = task.map(TaskId);
        a
    }

    #[test]
    fn insert_then_get_round_trips_for_copied_and_linked() {
        let conn = setup();
        for (id, linked) in [(1, false), (2, true)] {
            let a = attachment(id, 1, Some(1), linked);
            AttachmentRepo::insert(&conn, &a).unwrap();
            assert_eq!(
                AttachmentRepo::get(&conn, AttachmentId(id)).unwrap(),
                Some(a)
            );
        }
    }

    #[test]
    fn lists_are_scoped_to_task_and_project() {
        let conn = setup();
        AttachmentRepo::insert(&conn, &attachment(1, 1, Some(1), false)).unwrap();
        AttachmentRepo::insert(&conn, &attachment(2, 1, None, false)).unwrap();
        AttachmentRepo::insert(&conn, &attachment(3, 2, Some(2), false)).unwrap();

        assert_eq!(
            AttachmentRepo::list_for_project(&conn, ProjectId(1))
                .unwrap()
                .len(),
            2
        );
        let for_task = AttachmentRepo::list_for_task(&conn, TaskId(1)).unwrap();
        assert_eq!(for_task.len(), 1);
        assert_eq!(for_task[0].id, AttachmentId(1));
    }

    #[test]
    fn rejects_a_task_from_another_project() {
        let conn = setup();
        // Task 2 is in project 2; the attachment claims project 1.
        let bad = attachment(1, 1, Some(2), false);
        assert!(AttachmentRepo::insert(&conn, &bad).is_err());
        let unknown = attachment(2, 1, Some(99), false);
        assert!(AttachmentRepo::insert(&conn, &unknown).is_err());
        assert!(AttachmentRepo::list_for_project(&conn, ProjectId(1))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn deleting_a_task_cascades_to_its_attachment_rows() {
        let conn = setup();
        AttachmentRepo::insert(&conn, &attachment(1, 1, Some(1), false)).unwrap();
        AttachmentRepo::insert(&conn, &attachment(2, 1, None, false)).unwrap();
        TaskRepo::delete(&conn, TaskId(1)).unwrap();
        let left = AttachmentRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].task_id, None);
    }

    #[test]
    fn delete_removes_the_row() {
        let conn = setup();
        AttachmentRepo::insert(&conn, &attachment(1, 1, None, true)).unwrap();
        AttachmentRepo::delete(&conn, AttachmentId(1)).unwrap();
        assert!(AttachmentRepo::get(&conn, AttachmentId(1))
            .unwrap()
            .is_none());
    }
}
