//! SQLite-backed storage: connection setup, migrations and the plain-file
//! layout under the OS data directory. No GPUI imports here.

mod commit_links;
mod migrations;
mod notes;
mod paths;
mod projects;
mod repos;
mod tasks;

pub use commit_links::CommitLinkRepo;
pub use migrations::run_migrations;
pub use notes::NoteRepo;
pub use paths::WorkspacePaths;
pub use projects::ProjectRepo;
pub use repos::RepoRepo;
pub use tasks::TaskRepo;

use rusqlite::Connection;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Domain(#[from] anyhow::Error),
    /// A commit-link rule was violated (cross-project, duplicate, ...).
    /// Callers match on this to show a specific message.
    #[error(transparent)]
    CommitLink(#[from] devspace_core::CommitLinkError),
}

/// Opens a connection and applies pragmas required by CLAUDE.md: WAL mode
/// and foreign key enforcement. Does not run migrations by itself so tests
/// can open an in-memory db and call `run_migrations` explicitly.
pub fn open_connection(path: &std::path::Path) -> Result<Connection, StorageError> {
    let conn = Connection::open(path)?;
    configure_connection(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection, StorageError> {
    let conn = Connection::open_in_memory()?;
    configure_connection(&conn)?;
    Ok(conn)
}

fn configure_connection(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}
