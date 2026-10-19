use devspace_core::ProjectId;
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

/// The optional working folder of a project (where "Open in IDE" and new
/// terminals start). Absent means "use the first linked repo".
pub struct ProjectFolderRepo;

impl ProjectFolderRepo {
    pub fn get(conn: &Connection, project: ProjectId) -> Result<Option<String>, StorageError> {
        conn.query_row(
            "SELECT path FROM project_folders WHERE project_id = ?1",
            params![project.0],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
    }

    pub fn set(conn: &Connection, project: ProjectId, path: &str) -> Result<(), StorageError> {
        conn.execute(
            "INSERT INTO project_folders (project_id, path) VALUES (?1, ?2)
             ON CONFLICT(project_id) DO UPDATE SET path = excluded.path",
            params![project.0, path],
        )?;
        Ok(())
    }

    pub fn clear(conn: &Connection, project: ProjectId) -> Result<(), StorageError> {
        conn.execute(
            "DELETE FROM project_folders WHERE project_id = ?1",
            params![project.0],
        )?;
        Ok(())
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
    fn set_replaces_and_clear_removes() {
        let conn = setup();
        assert_eq!(ProjectFolderRepo::get(&conn, ProjectId(1)).unwrap(), None);

        ProjectFolderRepo::set(&conn, ProjectId(1), "D:/a").unwrap();
        ProjectFolderRepo::set(&conn, ProjectId(1), "D:/b").unwrap();
        assert_eq!(
            ProjectFolderRepo::get(&conn, ProjectId(1))
                .unwrap()
                .as_deref(),
            Some("D:/b")
        );

        ProjectFolderRepo::clear(&conn, ProjectId(1)).unwrap();
        assert_eq!(ProjectFolderRepo::get(&conn, ProjectId(1)).unwrap(), None);
    }

    #[test]
    fn deleting_the_project_removes_its_folder() {
        let conn = setup();
        ProjectFolderRepo::set(&conn, ProjectId(1), "D:/a").unwrap();
        ProjectRepo::delete(&conn, ProjectId(1)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_folders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }
}
