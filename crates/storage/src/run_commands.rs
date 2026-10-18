use devspace_core::{ProjectId, RunCommand, RunCommandId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

/// Persistence for a project's saved run commands. Only the definition is
/// stored; running state and logs live in memory in the runner.
pub struct RunCommandRepo;

impl RunCommandRepo {
    pub fn insert(conn: &Connection, cmd: &RunCommand) -> Result<(), StorageError> {
        conn.execute(
            "INSERT INTO run_commands (id, project_id, name, command, cwd) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![cmd.id.0, cmd.project_id.0, cmd.name, cmd.command, cmd.cwd],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, id: RunCommandId) -> Result<Option<RunCommand>, StorageError> {
        conn.query_row(
            "SELECT id, project_id, name, command, cwd FROM run_commands WHERE id = ?1",
            params![id.0],
            Self::from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    /// Commands of a project in creation order, so the list is stable.
    pub fn list_for_project(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<Vec<RunCommand>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, command, cwd FROM run_commands \
             WHERE project_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![project_id.0], Self::from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    /// Updates name, command and cwd; the owning project never changes.
    pub fn update(conn: &Connection, cmd: &RunCommand) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE run_commands SET name = ?2, command = ?3, cwd = ?4 WHERE id = ?1",
            params![cmd.id.0, cmd.name, cmd.command, cmd.cwd],
        )?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: RunCommandId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM run_commands WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<RunCommand> {
        Ok(RunCommand {
            id: RunCommandId(row.get(0)?),
            project_id: ProjectId(row.get(1)?),
            name: row.get(2)?,
            command: row.get(3)?,
            cwd: row.get(4)?,
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
        for id in [1, 2] {
            let project = Project::new(ProjectId(id), "P", 0).unwrap();
            ProjectRepo::insert(&conn, &project).unwrap();
        }
        conn
    }

    fn cmd(id: i64, project: i64, name: &str) -> RunCommand {
        RunCommand::new(RunCommandId(id), ProjectId(project), name, "cargo run").unwrap()
    }

    #[test]
    fn insert_then_get_round_trips_with_and_without_cwd() {
        let conn = setup();
        let mut a = cmd(1, 1, "a");
        a.cwd = Some("/work".into());
        RunCommandRepo::insert(&conn, &a).unwrap();
        RunCommandRepo::insert(&conn, &cmd(2, 1, "b")).unwrap();
        assert_eq!(
            RunCommandRepo::get(&conn, RunCommandId(1)).unwrap(),
            Some(a)
        );
        let b = RunCommandRepo::get(&conn, RunCommandId(2))
            .unwrap()
            .unwrap();
        assert_eq!(b.cwd, None);
        assert_eq!(RunCommandRepo::get(&conn, RunCommandId(9)).unwrap(), None);
    }

    #[test]
    fn list_is_per_project_and_ordered_by_id() {
        let conn = setup();
        RunCommandRepo::insert(&conn, &cmd(3, 1, "third")).unwrap();
        RunCommandRepo::insert(&conn, &cmd(1, 1, "first")).unwrap();
        RunCommandRepo::insert(&conn, &cmd(2, 2, "other")).unwrap();
        let names: Vec<_> = RunCommandRepo::list_for_project(&conn, ProjectId(1))
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["first", "third"]);
    }

    #[test]
    fn update_changes_fields_and_delete_removes() {
        let conn = setup();
        let mut a = cmd(1, 1, "a");
        RunCommandRepo::insert(&conn, &a).unwrap();
        a.name = "renamed".into();
        a.command = "npm test".into();
        a.cwd = Some("/x".into());
        RunCommandRepo::update(&conn, &a).unwrap();
        assert_eq!(RunCommandRepo::get(&conn, a.id).unwrap(), Some(a.clone()));
        RunCommandRepo::delete(&conn, a.id).unwrap();
        assert_eq!(RunCommandRepo::get(&conn, a.id).unwrap(), None);
    }

    #[test]
    fn deleting_a_project_cascades_to_its_commands() {
        let conn = setup();
        RunCommandRepo::insert(&conn, &cmd(1, 1, "a")).unwrap();
        conn.execute("DELETE FROM projects WHERE id = 1", [])
            .unwrap();
        assert!(RunCommandRepo::list_for_project(&conn, ProjectId(1))
            .unwrap()
            .is_empty());
    }
}
