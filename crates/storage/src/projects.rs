use devspace_core::{Project, ProjectId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

/// SQLite-backed CRUD for `Project`. Takes `&Connection` rather than owning
/// one so callers control transactions and connection lifetime.
pub struct ProjectRepo;

impl ProjectRepo {
    pub fn insert(conn: &Connection, project: &Project) -> Result<(), StorageError> {
        conn.execute(
            "INSERT INTO projects (id, name, description, archived, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.id.0,
                project.name,
                project.description,
                project.archived,
                project.created_at,
                project.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn update(conn: &Connection, project: &Project) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE projects SET name = ?2, description = ?3, archived = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                project.id.0,
                project.name,
                project.description,
                project.archived,
                project.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, id: ProjectId) -> Result<Option<Project>, StorageError> {
        conn.query_row(
            "SELECT id, name, description, archived, created_at, updated_at
             FROM projects WHERE id = ?1",
            params![id.0],
            Self::from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    pub fn list(conn: &Connection, include_archived: bool) -> Result<Vec<Project>, StorageError> {
        let sql = if include_archived {
            "SELECT id, name, description, archived, created_at, updated_at FROM projects
             ORDER BY name"
        } else {
            "SELECT id, name, description, archived, created_at, updated_at FROM projects
             WHERE archived = 0 ORDER BY name"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], Self::from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete(conn: &Connection, id: ProjectId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Project> {
        Ok(Project {
            id: ProjectId(row.get(0)?),
            name: row.get(1)?,
            description: row.get(2)?,
            archived: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrations::run_migrations, open_in_memory};

    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        conn
    }

    #[test]
    fn insert_then_get_round_trips() {
        let conn = setup();
        let project = Project::new(ProjectId(1), "Devspace", 100).unwrap();
        ProjectRepo::insert(&conn, &project).unwrap();

        let fetched = ProjectRepo::get(&conn, ProjectId(1)).unwrap().unwrap();
        assert_eq!(fetched, project);
    }

    #[test]
    fn get_missing_returns_none() {
        let conn = setup();
        assert!(ProjectRepo::get(&conn, ProjectId(42)).unwrap().is_none());
    }

    #[test]
    fn list_excludes_archived_by_default() {
        let conn = setup();
        let mut archived = Project::new(ProjectId(1), "Old", 0).unwrap();
        archived.archive(1);
        let active = Project::new(ProjectId(2), "Active", 0).unwrap();
        ProjectRepo::insert(&conn, &archived).unwrap();
        ProjectRepo::insert(&conn, &active).unwrap();

        let visible = ProjectRepo::list(&conn, false).unwrap();
        assert_eq!(visible, vec![active]);

        let all = ProjectRepo::list(&conn, true).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn update_persists_changes() {
        let conn = setup();
        let mut project = Project::new(ProjectId(1), "Devspace", 0).unwrap();
        ProjectRepo::insert(&conn, &project).unwrap();

        project.rename("Renamed", 50).unwrap();
        ProjectRepo::update(&conn, &project).unwrap();

        let fetched = ProjectRepo::get(&conn, ProjectId(1)).unwrap().unwrap();
        assert_eq!(fetched.name, "Renamed");
        assert_eq!(fetched.updated_at, 50);
    }

    #[test]
    fn delete_removes_row() {
        let conn = setup();
        let project = Project::new(ProjectId(1), "Devspace", 0).unwrap();
        ProjectRepo::insert(&conn, &project).unwrap();
        ProjectRepo::delete(&conn, ProjectId(1)).unwrap();
        assert!(ProjectRepo::get(&conn, ProjectId(1)).unwrap().is_none());
    }
}
