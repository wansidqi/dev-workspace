use devspace_core::{ProjectId, Repo, RepoId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

pub struct RepoRepo;

impl RepoRepo {
    pub fn insert(conn: &Connection, repo: &Repo) -> Result<(), StorageError> {
        conn.execute(
            "INSERT INTO repos (id, project_id, path, name) VALUES (?1, ?2, ?3, ?4)",
            params![repo.id.0, repo.project_id.0, repo.path, repo.name],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, id: RepoId) -> Result<Option<Repo>, StorageError> {
        conn.query_row(
            "SELECT id, project_id, path, name FROM repos WHERE id = ?1",
            params![id.0],
            Self::from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    pub fn list_for_project(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<Vec<Repo>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, path, name FROM repos WHERE project_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![project_id.0], Self::from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete(conn: &Connection, id: RepoId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM repos WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Repo> {
        Ok(Repo {
            id: RepoId(row.get(0)?),
            project_id: ProjectId(row.get(1)?),
            path: row.get(2)?,
            name: row.get(3)?,
        })
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
        let repo = Repo::new(RepoId(1), ProjectId(1), "/some/path", "myrepo").unwrap();
        RepoRepo::insert(&conn, &repo).unwrap();
        let fetched = RepoRepo::get(&conn, RepoId(1)).unwrap().unwrap();
        assert_eq!(fetched, repo);
    }

    #[test]
    fn list_for_project_returns_all_linked_repos() {
        let conn = setup();
        let a = Repo::new(RepoId(1), ProjectId(1), "/a", "a").unwrap();
        let b = Repo::new(RepoId(2), ProjectId(1), "/b", "b").unwrap();
        RepoRepo::insert(&conn, &a).unwrap();
        RepoRepo::insert(&conn, &b).unwrap();

        let repos = RepoRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(repos.len(), 2);
    }

    #[test]
    fn delete_removes_repo() {
        let conn = setup();
        let repo = Repo::new(RepoId(1), ProjectId(1), "/a", "a").unwrap();
        RepoRepo::insert(&conn, &repo).unwrap();
        RepoRepo::delete(&conn, RepoId(1)).unwrap();
        assert!(RepoRepo::get(&conn, RepoId(1)).unwrap().is_none());
    }
}
