//! Workspace backup and single-project export. SQLite itself is the
//! container (no archive/zip dependency): a backup is `VACUUM INTO` plus the
//! `projects/` file tree, and an export is a folder holding a fresh
//! `project.db` with only that project's rows plus the files its notes and
//! attachments reference. See [`crate::import`] for the reverse direction.

use std::fs;
use std::path::{Path, PathBuf};

use devspace_core::{is_safe_relative_path, sanitize_file_name, ProjectId};
use rusqlite::{params, Connection};

use crate::{run_migrations, StorageError, WorkspacePaths};

/// File name of the exported database inside an export folder.
pub const EXPORT_DB_NAME: &str = "project.db";
/// Suffix of an export folder, so users (and the importer) can recognise one.
pub const EXPORT_DIR_SUFFIX: &str = ".devspace-export";

/// What a workspace backup produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    /// The timestamped folder under `backups/`.
    pub dir: PathBuf,
    /// Number of project files copied (the database is extra).
    pub files: usize,
}

/// What a project export produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// The `<name>.devspace-export` folder.
    pub dir: PathBuf,
    /// Note and attachment files copied into it.
    pub files: usize,
    /// Referenced files that were missing on disk (their rows still export).
    pub missing_files: usize,
}

pub(crate) fn invalid(message: impl Into<String>) -> StorageError {
    StorageError::Domain(anyhow::anyhow!(message.into()))
}

/// Formats Unix seconds as `YYYYMMDD-HHMMSS` (UTC). Hand-rolled (civil from
/// days) so storage needs no date crate; used only for folder names.
pub(crate) fn format_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Creates `parent/<base>` (or `<base>-2`, `-3`, ...) and returns it. Never
/// reuses an existing folder, so one run can never overwrite another.
fn create_unique_dir(parent: &Path, base: &str) -> Result<PathBuf, StorageError> {
    fs::create_dir_all(parent)?;
    for n in 1..1000 {
        let name = if n == 1 {
            base.to_string()
        } else {
            let (stem, suffix) = match base.strip_suffix(EXPORT_DIR_SUFFIX) {
                Some(stem) => (stem, EXPORT_DIR_SUFFIX),
                None => (base, ""),
            };
            format!("{stem}-{n}{suffix}")
        };
        let candidate = parent.join(name);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(invalid("could not find a free folder name"))
}

/// Copies a directory tree. Symlinks are skipped, never followed, so a
/// planted link cannot make a backup pull in files from elsewhere.
fn copy_tree(src: &Path, dst: &Path, files: &mut usize) -> Result<(), StorageError> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &to, files)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &to)?;
            *files += 1;
        }
    }
    Ok(())
}

/// Backs up the whole workspace into `<root>/backups/<timestamp>/`:
/// `app.db` (a consistent copy made by SQLite's `VACUUM INTO`, safe while
/// the app has the database open in WAL mode) and the `projects/` files.
/// `now` is Unix seconds, passed in so tests are deterministic.
pub fn backup_workspace(
    conn: &Connection,
    paths: &WorkspacePaths,
    now: i64,
) -> Result<BackupReport, StorageError> {
    let dir = create_unique_dir(&paths.backups_dir(), &format_stamp(now))?;
    let result = (|| {
        // The target path is a bound parameter, so quotes in it cannot
        // break out of the statement.
        let target = dir.join("app.db");
        let target = target
            .to_str()
            .ok_or_else(|| invalid("backup path is not valid unicode"))?;
        conn.execute("VACUUM INTO ?1", params![target])?;
        let mut files = 0;
        let projects = paths.root().join("projects");
        if projects.is_dir() {
            copy_tree(&projects, &dir.join("projects"), &mut files)?;
        }
        Ok(BackupReport {
            dir: dir.clone(),
            files,
        })
    })();
    if result.is_err() {
        // Don't leave a half-written backup that looks complete.
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

/// Copies every row of `table` matching `where_sql` (one `?1` project-id
/// parameter) from `src` to `dst`, column for column. The tables share a
/// schema because `dst` was built by the same migrations.
fn copy_rows(
    src: &Connection,
    dst: &Connection,
    table: &str,
    where_sql: &str,
    project: i64,
) -> Result<(), StorageError> {
    let mut select = src.prepare(&format!("SELECT * FROM {table} WHERE {where_sql}"))?;
    let columns = select.column_count();
    let placeholders = (1..=columns)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut insert = dst.prepare(&format!("INSERT INTO {table} VALUES ({placeholders})"))?;
    let mut rows = select.query(params![project])?;
    while let Some(row) = rows.next()? {
        let values = (0..columns)
            .map(|i| row.get::<_, rusqlite::types::Value>(i))
            .collect::<Result<Vec<_>, _>>()?;
        insert.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(())
}

pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool, StorageError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table],
        |row| row.get(0),
    )?)
}

/// Copies the files named by `rel_paths` (relative to `from_dir`) into
/// `to_dir`. Paths come from our own database but are still checked; a file
/// that is missing is counted, not fatal.
fn copy_referenced(
    from_dir: &Path,
    to_dir: &Path,
    rel_paths: &[String],
    report: &mut ExportReport,
) -> Result<(), StorageError> {
    for rel in rel_paths {
        if !is_safe_relative_path(rel) {
            return Err(invalid(format!("unsafe file path in database: {rel}")));
        }
        let from = from_dir.join(rel);
        let is_regular = fs::symlink_metadata(&from)
            .map(|m| m.is_file())
            .unwrap_or(false);
        if !is_regular {
            report.missing_files += 1;
            continue;
        }
        let to = to_dir.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&from, &to)?;
        report.files += 1;
    }
    Ok(())
}

/// Exports one project into `<dest_parent>/<project name>.devspace-export/`:
/// `project.db` (current schema, only this project's rows; credentials are
/// never included) and `files/notes`, `files/attachments`. Linked
/// attachments export their metadata only (the original stays where it is).
pub fn export_project(
    conn: &Connection,
    paths: &WorkspacePaths,
    project: ProjectId,
    dest_parent: &Path,
) -> Result<ExportReport, StorageError> {
    let name: String = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            params![project.0],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => invalid("project not found"),
            other => other.into(),
        })?;
    let base = format!("{}{EXPORT_DIR_SUFFIX}", sanitize_file_name(&name));
    let dir = create_unique_dir(dest_parent, &base)?;
    let mut report = ExportReport {
        dir: dir.clone(),
        files: 0,
        missing_files: 0,
    };
    let result = write_export(conn, paths, project, &dir, &mut report);
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result.map(|()| report)
}

fn write_export(
    src: &Connection,
    paths: &WorkspacePaths,
    project: ProjectId,
    dir: &Path,
    report: &mut ExportReport,
) -> Result<(), StorageError> {
    let p = project.0;
    // One read snapshot so rows and file lists agree with each other.
    let snapshot = src.unchecked_transaction()?;
    {
        // Plain rollback-journal mode (no -wal/-shm side files), so the
        // export folder is a single self-contained, portable file.
        let mut dst = Connection::open(dir.join(EXPORT_DB_NAME))?;
        run_migrations(&mut dst)?;
        // Tasks may be copied before their parents; integrity is checked below.
        dst.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let tx = dst.transaction()?;
        let by_task = "task_id IN (SELECT id FROM tasks WHERE project_id = ?1)";
        copy_rows(&snapshot, &tx, "projects", "id = ?1", p)?;
        copy_rows(&snapshot, &tx, "repos", "project_id = ?1", p)?;
        copy_rows(&snapshot, &tx, "tasks", "project_id = ?1", p)?;
        copy_rows(&snapshot, &tx, "task_tags", by_task, p)?;
        copy_rows(&snapshot, &tx, "notes", "project_id = ?1", p)?;
        copy_rows(&snapshot, &tx, "attachments", "project_id = ?1", p)?;
        copy_rows(&snapshot, &tx, "commit_links", by_task, p)?;
        if table_exists(&snapshot, "run_commands")? {
            copy_rows(&snapshot, &tx, "run_commands", "project_id = ?1", p)?;
        }
        tx.commit()?;
        let violations: i64 =
            dst.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if violations > 0 {
            return Err(invalid("project data has dangling references"));
        }
    }

    let note_files = string_column(
        &snapshot,
        "SELECT file_path FROM notes WHERE project_id = ?1",
        p,
    )?;
    let attachment_files = string_column(
        &snapshot,
        "SELECT file_path FROM attachments WHERE project_id = ?1 AND is_linked = 0",
        p,
    )?;
    snapshot.commit()?;
    copy_referenced(
        &paths.notes_dir(project),
        &dir.join("files").join("notes"),
        &note_files,
        report,
    )?;
    copy_referenced(
        &paths.attachments_dir(project),
        &dir.join("files").join("attachments"),
        &attachment_files,
        report,
    )?;
    fs::create_dir_all(dir.join("files").join("notes"))?;
    fs::create_dir_all(dir.join("files").join("attachments"))?;
    Ok(())
}

fn string_column(conn: &Connection, sql: &str, project: i64) -> Result<Vec<String>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    /// A scratch directory under the OS temp dir, removed on drop (storage
    /// has no `tempfile` dev-dependency and these tests need little).
    pub struct Scratch(pub PathBuf);

    impl Scratch {
        pub fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "devspace-storage-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Self(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Scratch;
    use super::*;
    use crate::open_in_memory;

    #[test]
    fn stamp_formats_known_dates() {
        assert_eq!(format_stamp(0), "19700101-000000");
        assert_eq!(format_stamp(1_000_000_000), "20010909-014640");
        assert_eq!(format_stamp(1_709_164_800 + 3661), "20240229-010101");
    }

    #[test]
    fn unique_dir_never_reuses_a_name() {
        let scratch = Scratch::new("uniq");
        let a = create_unique_dir(scratch.path(), "x.devspace-export").unwrap();
        let b = create_unique_dir(scratch.path(), "x.devspace-export").unwrap();
        assert_ne!(a, b);
        assert!(b.ends_with("x-2.devspace-export"));
    }

    #[test]
    fn backup_copies_database_and_project_files() {
        let scratch = Scratch::new("backup");
        let paths = WorkspacePaths::new(scratch.path().join("devspace"));
        paths.ensure_dirs(Some(ProjectId(1))).unwrap();
        let mut conn = crate::open_connection(&paths.database_file()).unwrap();
        run_migrations(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, archived, created_at, updated_at) VALUES (1, 'It''s', 0, 0, 0)",
            [],
        )
        .unwrap();
        fs::write(paths.notes_dir(ProjectId(1)).join("1.md"), "hello").unwrap();

        let report = backup_workspace(&conn, &paths, 1_000_000_000).unwrap();
        assert!(report.dir.ends_with("20010909-014640"));
        assert_eq!(report.files, 1);
        assert_eq!(
            fs::read_to_string(report.dir.join("projects/1/notes/1.md")).unwrap(),
            "hello"
        );
        let copy = Connection::open(report.dir.join("app.db")).unwrap();
        let name: String = copy
            .query_row("SELECT name FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "It's");

        // A second backup in the same second gets its own folder.
        let again = backup_workspace(&conn, &paths, 1_000_000_000).unwrap();
        assert_ne!(again.dir, report.dir);
    }

    #[test]
    fn backup_path_with_quotes_is_safe() {
        let scratch = Scratch::new("it's");
        let paths = WorkspacePaths::new(scratch.path().join("dev'space"));
        paths.ensure_dirs(None).unwrap();
        let conn = open_in_memory().unwrap();
        let report = backup_workspace(&conn, &paths, 5).unwrap();
        assert!(report.dir.join("app.db").is_file());
    }

    #[test]
    fn export_of_unknown_project_fails_cleanly() {
        let scratch = Scratch::new("noproj");
        let paths = WorkspacePaths::new(scratch.path().join("devspace"));
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        assert!(export_project(&conn, &paths, ProjectId(9), scratch.path()).is_err());
    }
}
