//! Cross-project title search for the command palette.
//!
//! Lives apart from the per-entity repos so the palette's needs (read-only,
//! all non-archived projects, bounded result count) stay in one place. No
//! schema changes: it only reads `tasks`, `notes` and `projects`.

use devspace_core::{NoteId, ProjectId, TaskId};
use rusqlite::{params_from_iter, types::Value, Connection};

use crate::{notes::NoteRepo, tasks::TaskRepo, StorageError};

/// Character that escapes `%` and `_` inside generated LIKE patterns.
const ESCAPE: char = '\\';
/// More words than this add cost but no useful precision.
const MAX_TOKENS: usize = 6;
/// Longest single word considered; longer input is truncated, not rejected.
const MAX_TOKEN_CHARS: usize = 100;

/// A task whose title matched, with enough context to open it from any
/// project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHit {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub project_name: String,
    pub title: String,
}

/// A note whose title matched, with enough context to open it from any
/// project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteHit {
    pub note_id: NoteId,
    pub project_id: ProjectId,
    pub project_name: String,
    pub task_id: Option<TaskId>,
    pub title: String,
}

/// Escapes `%`, `_` and the escape character so user text is matched
/// literally by LIKE.
pub fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '%' || c == '_' || c == ESCAPE {
            out.push(ESCAPE);
        }
        out.push(c);
    }
    out
}

/// One `%word%` pattern per whitespace-separated word (all must match, in
/// any order). Empty input yields no patterns, which means "match all".
/// SQLite's LIKE is case-insensitive for ASCII only; the palette's Rust
/// ranker does the Unicode-aware ordering on top of this prefilter.
fn patterns(text: &str) -> Vec<String> {
    text.split_whitespace()
        .take(MAX_TOKENS)
        .map(|w| {
            let w: String = w.chars().take(MAX_TOKEN_CHARS).collect();
            format!("%{}%", escape_like(&w))
        })
        .collect()
}

/// Builds `AND col LIKE ?N ESCAPE '\'` clauses (placeholders only; the user
/// text is always bound, never spliced into the SQL).
fn like_clauses(column: &str, count: usize) -> String {
    (0..count)
        .map(|i| format!(" AND {column} LIKE ?{} ESCAPE '{ESCAPE}'", i + 1))
        .collect()
}

fn bind(pats: Vec<String>, limit: usize) -> Vec<Value> {
    let mut values: Vec<Value> = pats.into_iter().map(Value::Text).collect();
    values.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
    values
}

impl TaskRepo {
    /// Tasks of every non-archived project whose title contains every word
    /// of `text`, most recently updated first, at most `limit`. Empty `text`
    /// returns the most recently updated tasks (the palette's "recent").
    pub fn search_titles(
        conn: &Connection,
        text: &str,
        limit: usize,
    ) -> Result<Vec<TaskHit>, StorageError> {
        let pats = patterns(text);
        let sql = format!(
            "SELECT t.id, t.project_id, p.name, t.title
             FROM tasks t JOIN projects p ON p.id = t.project_id
             WHERE p.archived = 0{}
             ORDER BY t.updated_at DESC, t.id DESC LIMIT ?{}",
            like_clauses("t.title", pats.len()),
            pats.len() + 1
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(bind(pats, limit)), |row| {
            Ok(TaskHit {
                task_id: TaskId(row.get(0)?),
                project_id: ProjectId(row.get(1)?),
                project_name: row.get(2)?,
                title: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }
}

impl NoteRepo {
    /// Notes of every non-archived project whose title contains every word
    /// of `text`, most recently updated first, at most `limit`. Empty `text`
    /// returns the most recently updated notes. Titles only: note content
    /// lives in plain files and is not scanned here.
    pub fn search_titles(
        conn: &Connection,
        text: &str,
        limit: usize,
    ) -> Result<Vec<NoteHit>, StorageError> {
        let pats = patterns(text);
        let sql = format!(
            "SELECT n.id, n.project_id, p.name, n.task_id, n.title
             FROM notes n JOIN projects p ON p.id = n.project_id
             WHERE p.archived = 0{}
             ORDER BY n.updated_at DESC, n.id DESC LIMIT ?{}",
            like_clauses("n.title", pats.len()),
            pats.len() + 1
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(bind(pats, limit)), |row| {
            Ok(NoteHit {
                note_id: NoteId(row.get(0)?),
                project_id: ProjectId(row.get(1)?),
                project_name: row.get(2)?,
                task_id: row.get::<_, Option<i64>>(3)?.map(TaskId),
                title: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrations::run_migrations, open_in_memory, projects::ProjectRepo};
    use devspace_core::{Note, NoteKind, Project, Task};

    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for (id, name) in [(1, "Alpha"), (2, "Beta")] {
            ProjectRepo::insert(&conn, &Project::new(ProjectId(id), name, 0).unwrap()).unwrap();
        }
        conn
    }

    fn add_task(conn: &Connection, id: i64, project: i64, title: &str, updated: i64) {
        let mut t = Task::new(TaskId(id), ProjectId(project), title, 0).unwrap();
        t.updated_at = updated;
        TaskRepo::insert(conn, &t).unwrap();
    }

    fn add_note(conn: &Connection, id: i64, project: i64, title: &str) {
        let note = Note {
            id: NoteId(id),
            project_id: ProjectId(project),
            task_id: None,
            kind: NoteKind::Note,
            title: title.into(),
            file_path: format!("{id}.md"),
            updated_at: id,
        };
        NoteRepo::insert(conn, &note).unwrap();
    }

    fn titles(hits: &[TaskHit]) -> Vec<&str> {
        hits.iter().map(|h| h.title.as_str()).collect()
    }

    #[test]
    fn escape_like_escapes_wildcards_and_escape_char() {
        assert_eq!(escape_like("a%b_c\\d"), "a\\%b\\_c\\\\d");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn searches_across_projects_and_skips_archived() {
        let conn = setup();
        add_task(&conn, 1, 1, "Fix login bug", 10);
        add_task(&conn, 2, 2, "Login page", 20);
        add_task(&conn, 3, 2, "Unrelated", 30);
        let hits = TaskRepo::search_titles(&conn, "LOGIN", 10).unwrap();
        assert_eq!(titles(&hits), vec!["Login page", "Fix login bug"]);
        assert_eq!(hits[0].project_name, "Beta");

        let mut p = ProjectRepo::get(&conn, ProjectId(2)).unwrap().unwrap();
        p.archived = true;
        ProjectRepo::update(&conn, &p).unwrap();
        let hits = TaskRepo::search_titles(&conn, "login", 10).unwrap();
        assert_eq!(titles(&hits), vec!["Fix login bug"]);
    }

    #[test]
    fn percent_and_underscore_are_literal() {
        let conn = setup();
        add_task(&conn, 1, 1, "100% done", 1);
        add_task(&conn, 2, 1, "snake_case", 2);
        add_task(&conn, 3, 1, "snakeXcase", 3);
        add_task(&conn, 4, 1, "anything", 4);
        assert_eq!(
            titles(&TaskRepo::search_titles(&conn, "%", 10).unwrap()),
            vec!["100% done"]
        );
        assert_eq!(
            titles(&TaskRepo::search_titles(&conn, "snake_case", 10).unwrap()),
            vec!["snake_case"]
        );
        assert_eq!(
            titles(&TaskRepo::search_titles(&conn, "_", 10).unwrap()),
            vec!["snake_case"]
        );
    }

    #[test]
    fn backslash_and_quotes_cannot_break_the_query() {
        let conn = setup();
        add_task(&conn, 1, 1, "it's a \\ test", 1);
        add_task(&conn, 2, 1, "other", 2);
        for evil in [
            "'",
            "\\",
            "' OR 1=1 --",
            "\"; DROP TABLE tasks; --",
            "%' --",
        ] {
            let hits = TaskRepo::search_titles(&conn, evil, 10).unwrap();
            assert!(hits.len() <= 1, "{evil:?} matched too much");
        }
        assert_eq!(
            titles(&TaskRepo::search_titles(&conn, "it's", 10).unwrap()),
            vec!["it's a \\ test"]
        );
        // Tables survived.
        assert_eq!(TaskRepo::search_titles(&conn, "", 10).unwrap().len(), 2);
    }

    #[test]
    fn words_match_in_any_order_and_limit_applies() {
        let conn = setup();
        add_task(&conn, 1, 1, "Fix login bug", 1);
        add_task(&conn, 2, 1, "Bug in login form", 2);
        add_task(&conn, 3, 1, "Login only", 3);
        let hits = TaskRepo::search_titles(&conn, "bug login", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(TaskRepo::search_titles(&conn, "", 2).unwrap().len(), 2);
        assert!(TaskRepo::search_titles(&conn, "login", 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn empty_text_returns_recent_first() {
        let conn = setup();
        add_task(&conn, 1, 1, "old", 1);
        add_task(&conn, 2, 1, "new", 9);
        let hits = TaskRepo::search_titles(&conn, "   ", 10).unwrap();
        assert_eq!(titles(&hits), vec!["new", "old"]);
    }

    #[test]
    fn note_search_is_cross_project_and_literal() {
        let conn = setup();
        add_note(&conn, 1, 1, "Deploy steps");
        add_note(&conn, 2, 2, "deploy 100% checklist");
        add_note(&conn, 3, 2, "other");
        let hits = NoteRepo::search_titles(&conn, "deploy", 10).unwrap();
        assert_eq!(hits.len(), 2);
        let hits = NoteRepo::search_titles(&conn, "%", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].project_id, ProjectId(2));
        assert!(NoteRepo::search_titles(&conn, "x' OR '1'='1", 10)
            .unwrap()
            .is_empty());
    }
}
