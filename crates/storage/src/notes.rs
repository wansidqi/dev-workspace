use devspace_core::{Note, NoteId, NoteKind, ProjectId, TaskId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

pub struct NoteRepo;

impl NoteRepo {
    pub fn insert(conn: &Connection, note: &Note) -> Result<(), StorageError> {
        conn.execute(
            "INSERT INTO notes (id, project_id, task_id, kind, title, file_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                note.id.0,
                note.project_id.0,
                note.task_id.map(|id| id.0),
                kind_to_str(note.kind),
                note.title,
                note.file_path,
                note.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn update(conn: &Connection, note: &Note) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE notes SET task_id = ?2, kind = ?3, title = ?4, file_path = ?5, updated_at = ?6
             WHERE id = ?1",
            params![
                note.id.0,
                note.task_id.map(|id| id.0),
                kind_to_str(note.kind),
                note.title,
                note.file_path,
                note.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, id: NoteId) -> Result<Option<Note>, StorageError> {
        conn.query_row(
            "SELECT id, project_id, task_id, kind, title, file_path, updated_at
             FROM notes WHERE id = ?1",
            params![id.0],
            Self::from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    pub fn list_for_project(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<Vec<Note>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, task_id, kind, title, file_path, updated_at
             FROM notes WHERE project_id = ?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id.0], Self::from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn list_for_task(conn: &Connection, task_id: TaskId) -> Result<Vec<Note>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, task_id, kind, title, file_path, updated_at
             FROM notes WHERE task_id = ?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![task_id.0], Self::from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn delete(conn: &Connection, id: NoteId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Note> {
        let kind: String = row.get(3)?;
        Ok(Note {
            id: NoteId(row.get(0)?),
            project_id: ProjectId(row.get(1)?),
            task_id: row.get::<_, Option<i64>>(2)?.map(TaskId),
            kind: str_to_kind(&kind),
            title: row.get(4)?,
            file_path: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }
}

fn kind_to_str(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Note => "note",
        NoteKind::Script => "script",
        NoteKind::Sql => "sql",
    }
}

fn str_to_kind(s: &str) -> NoteKind {
    match s {
        "script" => NoteKind::Script,
        "sql" => NoteKind::Sql,
        _ => NoteKind::Note,
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
        let note = Note::new(
            NoteId(1),
            ProjectId(1),
            NoteKind::Note,
            "Todo",
            "todo.md",
            100,
        )
        .unwrap();
        NoteRepo::insert(&conn, &note).unwrap();
        let fetched = NoteRepo::get(&conn, NoteId(1)).unwrap().unwrap();
        assert_eq!(fetched, note);
    }

    #[test]
    fn list_for_project_orders_by_updated_at_desc() {
        let conn = setup();
        let a = Note::new(NoteId(1), ProjectId(1), NoteKind::Note, "a", "a.md", 100).unwrap();
        let b = Note::new(NoteId(2), ProjectId(1), NoteKind::Note, "b", "b.md", 200).unwrap();
        NoteRepo::insert(&conn, &a).unwrap();
        NoteRepo::insert(&conn, &b).unwrap();

        let notes = NoteRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(notes[0].title, "b");
        assert_eq!(notes[1].title, "a");
    }

    #[test]
    fn update_persists_title_change() {
        let conn = setup();
        let mut note = Note::new(NoteId(1), ProjectId(1), NoteKind::Note, "t", "t.md", 0).unwrap();
        NoteRepo::insert(&conn, &note).unwrap();

        note.title = "renamed".to_string();
        note.updated_at = 50;
        NoteRepo::update(&conn, &note).unwrap();

        let fetched = NoteRepo::get(&conn, NoteId(1)).unwrap().unwrap();
        assert_eq!(fetched.title, "renamed");
    }

    #[test]
    fn delete_removes_note() {
        let conn = setup();
        let note = Note::new(NoteId(1), ProjectId(1), NoteKind::Note, "t", "t.md", 0).unwrap();
        NoteRepo::insert(&conn, &note).unwrap();
        NoteRepo::delete(&conn, NoteId(1)).unwrap();
        assert!(NoteRepo::get(&conn, NoteId(1)).unwrap().is_none());
    }
}
