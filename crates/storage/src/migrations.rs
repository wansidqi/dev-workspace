use rusqlite::Connection;

use crate::StorageError;

/// Each migration is a numbered, immutable step. Never edit an applied
/// migration in place (CLAUDE.md storage rules) — append a new one instead.
const MIGRATIONS: &[(i64, &str)] = &[
    (
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
    ),
    (
        2,
        r#"
        -- A task links a given commit at most once, but one commit may be
        -- linked to many tasks (many-to-many). A hash is only unique within
        -- a repo, so repo_id is part of the key.
        CREATE UNIQUE INDEX idx_commit_links_unique
            ON commit_links(task_id, repo_id, hash);

        -- Project scoping lives in the database too, not just the UI or the
        -- repository code: a link is valid only if the task's project equals
        -- the repo's project.
        CREATE TRIGGER commit_links_same_project_insert
        BEFORE INSERT ON commit_links
        WHEN (SELECT project_id FROM tasks WHERE id = NEW.task_id)
             IS NOT (SELECT project_id FROM repos WHERE id = NEW.repo_id)
        BEGIN
            SELECT RAISE(ABORT, 'commit link crosses projects');
        END;

        CREATE TRIGGER commit_links_same_project_update
        BEFORE UPDATE OF task_id, repo_id ON commit_links
        WHEN (SELECT project_id FROM tasks WHERE id = NEW.task_id)
             IS NOT (SELECT project_id FROM repos WHERE id = NEW.repo_id)
        BEGIN
            SELECT RAISE(ABORT, 'commit link crosses projects');
        END;
        "#,
    ),
    (
        3,
        r#"
        -- Task tags. One row per (task, tag); the primary key makes a tag
        -- unique per task ignoring case ("Bug" and "bug" are the same tag).
        -- Deleting a task deletes its tags.
        CREATE TABLE task_tags (
            task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            tag     TEXT    NOT NULL COLLATE NOCASE,
            PRIMARY KEY (task_id, tag)
        );

        -- "Which tasks have this tag" (the tag filter).
        CREATE INDEX idx_task_tags_tag ON task_tags(tag);
        "#,
    ),
    (
        4,
        r#"
        -- Monthly timesheet (replaces the old standalone git-sheet JSON files).
        -- One settings row for the whole workspace.
        CREATE TABLE timesheet_settings (
            id                   INTEGER PRIMARY KEY CHECK (id = 1),
            name                 TEXT NOT NULL,
            role                 TEXT NOT NULL,
            title                TEXT NOT NULL,
            authors              TEXT NOT NULL,
            min_rows             INTEGER NOT NULL,
            est_base             REAL NOT NULL,
            est_line_factor      REAL NOT NULL,
            est_per_extra_commit REAL NOT NULL,
            est_per_extra_day    REAL NOT NULL,
            est_max_per_day      REAL NOT NULL
        );

        -- A month has a row here once it has been saved; remarks are free text.
        CREATE TABLE timesheet_reports (
            month   TEXT PRIMARY KEY,
            remarks TEXT NOT NULL
        );

        -- One task line per (project, month, key). Deleting a project removes
        -- its lines.
        CREATE TABLE timesheet_rows (
            id         INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            month      TEXT NOT NULL,
            key        TEXT NOT NULL,
            title      TEXT NOT NULL,
            date_from  TEXT NOT NULL,
            date_to    TEXT NOT NULL,
            hours      REAL,
            auto       INTEGER NOT NULL,
            source     TEXT NOT NULL,
            position   INTEGER NOT NULL,
            UNIQUE (project_id, month, key)
        );

        -- Snapshot of the commits behind a git row (not a commit link: it must
        -- survive rebases and repo unlinking). A hash is only unique within a
        -- repo, so the repo name is part of the key.
        CREATE TABLE timesheet_commits (
            row_id   INTEGER NOT NULL REFERENCES timesheet_rows(id) ON DELETE CASCADE,
            repo     TEXT NOT NULL,
            hash     TEXT NOT NULL,
            subject  TEXT NOT NULL,
            date     TEXT NOT NULL,
            lines    INTEGER NOT NULL,
            PRIMARY KEY (row_id, repo, hash)
        );
        "#,
    ),
    (
        5,
        r#"
        -- Optional folder "Open in IDE" and new terminals start in, for
        -- projects whose repos live side by side under one parent. A
        -- separate table keeps `projects` (and its imports) unchanged.
        CREATE TABLE project_folders (
            project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
            path       TEXT NOT NULL
        );
        "#,
    ),
];

/// The newest schema version this build knows. Import refuses databases
/// made by a newer app than this.
pub fn latest_migration_version() -> i64 {
    MIGRATIONS.last().map(|(v, _)| *v).unwrap_or(0)
}

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
    fn migration_three_upgrades_a_database_that_already_has_data() {
        // Build a database exactly as it was at migration 2, with a task in
        // it, then let `run_migrations` apply what is missing.
        let mut conn = open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY);")
            .unwrap();
        for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 2) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO projects (id, name, archived, created_at, updated_at) VALUES (1, 'P', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, status, priority, created_at, updated_at)
             VALUES (1, 1, 'existing', 'todo', 'medium', 0, 0)",
            [],
        )
        .unwrap();
        assert!(
            conn.execute("INSERT INTO task_tags (task_id, tag) VALUES (1, 'x')", [])
                .is_err(),
            "task_tags must not exist before migration 3"
        );

        run_migrations(&mut conn).unwrap();

        conn.execute("INSERT INTO task_tags (task_id, tag) VALUES (1, 'Bug')", [])
            .unwrap();
        assert!(
            conn.execute("INSERT INTO task_tags (task_id, tag) VALUES (1, 'BUG')", [])
                .is_err(),
            "unique ignoring case"
        );
        let titles: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE title = 'existing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(titles, 1, "existing data survives the upgrade");
        let versions: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(versions, 5);
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
