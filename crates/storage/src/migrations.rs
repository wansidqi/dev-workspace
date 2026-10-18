use rusqlite::Connection;

use crate::StorageError;

/// Each migration is a numbered, immutable step. Never edit an applied
/// migration in place (CLAUDE.md storage rules) — append a new one instead.
const MIGRATIONS: &[(i64, &str)] = &[(
    1,
    r#"
        CREATE TABLE projects (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL,
            description TEXT,
            archived    INTEGER NOT NULL DEFAULT 0,
            created_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL
        );

        CREATE TABLE repos (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            path       TEXT NOT NULL,
            name       TEXT NOT NULL
        );

        CREATE TABLE tasks (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            parent_id  INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
            title      TEXT NOT NULL,
            status     TEXT NOT NULL,
            priority   TEXT NOT NULL,
            due_at     INTEGER,
            branch     TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE notes (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            task_id    INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
            kind       TEXT NOT NULL,
            title      TEXT NOT NULL,
            file_path  TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE attachments (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            task_id    INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
            name       TEXT NOT NULL,
            file_path  TEXT NOT NULL,
            is_linked  INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE credentials (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            label      TEXT NOT NULL,
            kind       TEXT NOT NULL,
            nonce      BLOB NOT NULL,
            ciphertext BLOB NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE commit_links (
            id         INTEGER PRIMARY KEY,
            task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            repo_id    INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
            hash       TEXT NOT NULL,
            patch_id   TEXT,
            status     TEXT NOT NULL,
            note       TEXT,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE run_commands (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            name       TEXT NOT NULL,
            command    TEXT NOT NULL,
            cwd        TEXT
        );

        CREATE INDEX idx_tasks_project ON tasks(project_id);
        CREATE INDEX idx_tasks_parent ON tasks(parent_id);
        CREATE INDEX idx_notes_project ON notes(project_id);
        CREATE INDEX idx_commit_links_task ON commit_links(task_id);
        CREATE INDEX idx_commit_links_hash ON commit_links(repo_id, hash);
        "#,
)];

/// Applies every migration not yet recorded in `schema_migrations`, in a
/// transaction per migration.
pub fn run_migrations(conn: &mut Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY);",
    )?;

    for (version, sql) in MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
            [version],
            |row| row.get(0),
        )?;
        if already_applied {
            continue;
        }

        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [version],
        )?;
        tx.commit()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    #[test]
    fn running_migrations_twice_is_a_no_op() {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        run_migrations(&mut conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();

        let result = conn.execute(
            "INSERT INTO tasks (id, project_id, title, status, priority, created_at, updated_at)
             VALUES (1, 999, 't', 'todo', 'medium', 0, 0)",
            [],
        );
        assert!(result.is_err());
    }
}
