//! Tests for [`super`] (the project importer). Kept in their own file so the
//! importer stays readable; included via `#[path]` so they can reach its
//! private helpers.

use super::*;
use crate::transfer::test_support::Scratch;
use crate::{export_project, open_connection, run_migrations};

struct Fixture {
    scratch: Scratch,
    paths: WorkspacePaths,
    conn: Connection,
}

/// A workspace with an unrelated project 1 and the populated project 7:
/// tasks at three depths, tags, notes, a copied and a linked attachment,
/// commit links, a run command and a credential.
fn fixture(tag: &str) -> Fixture {
    let scratch = Scratch::new(tag);
    let paths = WorkspacePaths::new(scratch.path().join("devspace"));
    paths.ensure_dirs(Some(ProjectId(7))).unwrap();
    let mut conn = open_connection(&paths.database_file()).unwrap();
    run_migrations(&mut conn).unwrap();
    conn.execute_batch(
        "INSERT INTO projects VALUES (1, 'Other', NULL, 0, 1, 1);
         INSERT INTO repos VALUES (1, 1, 'C:/other', 'other');
         INSERT INTO tasks VALUES (1, 1, NULL, 'unrelated', 'todo', 'low', NULL, NULL, 1, 1);
         INSERT INTO projects VALUES (7, 'Big one', 'desc', 0, 100, 200);
         INSERT INTO repos VALUES (5, 7, 'C:/code/api', 'api');
         INSERT INTO repos VALUES (6, 7, 'C:/code/web', 'web');
         INSERT INTO tasks VALUES (20, 7, NULL, 'root', 'in_progress', 'high', 5000, 'feat/root', 10, 11);
         INSERT INTO tasks VALUES (21, 7, 20, 'child', 'todo', 'medium', NULL, NULL, 12, 13);
         INSERT INTO tasks VALUES (22, 7, 21, 'grandchild', 'done', 'urgent', NULL, 'feat/deep', 14, 15);
         INSERT INTO task_tags VALUES (20, 'Bug');
         INSERT INTO task_tags VALUES (22, 'backend');
         INSERT INTO notes VALUES (30, 7, NULL, 'note', 'project note', '30.md', 50);
         INSERT INTO notes VALUES (31, 7, 22, 'sql', 'deep sql', '31.sql', 51);
         INSERT INTO attachments VALUES (40, 7, 21, 'spec.pdf', '40-spec.pdf', 0, 60);
         INSERT INTO attachments VALUES (41, 7, NULL, 'ext.txt', 'C:/docs/ext.txt', 1, 61);
         INSERT INTO commit_links VALUES (50, 20, 5, 'abc123', 'p1', 'deployed', 'n', 70);
         INSERT INTO commit_links VALUES (51, 22, 6, 'abc123', NULL, 'linked', NULL, 71);
         INSERT INTO run_commands VALUES (60, 7, 'build', 'cargo build', NULL);
         INSERT INTO credentials VALUES (80, 7, 'db password', 'password', x'00', x'ff', 1);",
    )
    .unwrap();
    fs::write(paths.notes_dir(ProjectId(7)).join("30.md"), "# hello").unwrap();
    fs::write(paths.notes_dir(ProjectId(7)).join("31.sql"), "select 1;").unwrap();
    fs::write(
        paths.attachments_dir(ProjectId(7)).join("40-spec.pdf"),
        b"%PDF-fake",
    )
    .unwrap();
    Fixture {
        scratch,
        paths,
        conn,
    }
}

fn export(f: &Fixture) -> PathBuf {
    let out = f.scratch.path().join("out");
    export_project(&f.conn, &f.paths, ProjectId(7), &out)
        .unwrap()
        .dir
}

fn tamper(dir: &Path, sql: &str) {
    let db = Connection::open(dir.join(EXPORT_DB_NAME)).unwrap();
    // A hostile exporter is not bound by our foreign keys.
    db.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    db.execute_batch(sql).unwrap();
}

/// Everything about a project that must survive a round trip, with ids
/// replaced by names so two projects can be compared.
fn shape(conn: &Connection, paths: &WorkspacePaths, project: i64) -> Vec<String> {
    let mut out = Vec::new();
    let mut q = |sql: &str, label: &str| {
        let mut stmt = conn.prepare(sql).unwrap();
        let rows = stmt
            .query_map(params![project], |r| {
                let n = r.as_ref().column_count();
                let mut cols = Vec::new();
                for i in 0..n {
                    let v: rusqlite::types::Value = r.get(i)?;
                    cols.push(format!("{v:?}"));
                }
                Ok(cols.join("|"))
            })
            .unwrap();
        for row in rows {
            out.push(format!("{label}: {}", row.unwrap()));
        }
    };
    q(
        "SELECT name, description, archived, created_at, updated_at FROM projects WHERE id = ?1",
        "project",
    );
    q(
        "SELECT path, name FROM repos WHERE project_id = ?1 ORDER BY name",
        "repo",
    );
    q(
        "SELECT t.title, p.title, t.status, t.priority, t.due_at, t.branch, t.created_at, t.updated_at
         FROM tasks t LEFT JOIN tasks p ON p.id = t.parent_id
         WHERE t.project_id = ?1 ORDER BY t.title",
        "task",
    );
    q(
        "SELECT t.title, g.tag FROM task_tags g JOIN tasks t ON t.id = g.task_id
         WHERE t.project_id = ?1 ORDER BY t.title, g.tag",
        "tag",
    );
    q(
        "SELECT n.title, n.kind, n.updated_at, t.title FROM notes n
         LEFT JOIN tasks t ON t.id = n.task_id WHERE n.project_id = ?1 ORDER BY n.title",
        "note",
    );
    q(
        "SELECT a.name, a.is_linked, a.created_at, t.title FROM attachments a
         LEFT JOIN tasks t ON t.id = a.task_id WHERE a.project_id = ?1 ORDER BY a.name",
        "attachment",
    );
    q(
        "SELECT t.title, r.name, l.hash, l.patch_id, l.status, l.note, l.created_at
         FROM commit_links l JOIN tasks t ON t.id = l.task_id JOIN repos r ON r.id = l.repo_id
         WHERE t.project_id = ?1 ORDER BY t.title, r.name",
        "link",
    );
    q(
        "SELECT name, command, cwd FROM run_commands WHERE project_id = ?1",
        "run",
    );
    // File contents, through the stored (new) paths.
    let mut stmt = conn
        .prepare("SELECT title, file_path FROM notes WHERE project_id = ?1 ORDER BY title")
        .unwrap();
    let notes: Vec<(String, String)> = stmt
        .query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for (title, file) in notes {
        let text = fs::read_to_string(paths.notes_dir(ProjectId(project)).join(file)).unwrap();
        out.push(format!("notefile {title}: {text}"));
    }
    let stored: String = conn
        .query_row(
            "SELECT file_path FROM attachments WHERE project_id = ?1 AND is_linked = 0",
            params![project],
            |r| r.get(0),
        )
        .unwrap();
    out.push(format!(
        "pdf: {:?}",
        fs::read(paths.attachments_dir(ProjectId(project)).join(stored)).unwrap()
    ));
    out
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn export_has_only_that_project_and_never_credentials() {
    let f = fixture("exp");
    let dir = export(&f);
    assert!(dir.ends_with("Big one.devspace-export"));
    let db = Connection::open(dir.join(EXPORT_DB_NAME)).unwrap();
    assert_eq!(count(&db, "projects"), 1);
    assert_eq!(count(&db, "tasks"), 3);
    assert_eq!(count(&db, "repos"), 2);
    assert_eq!(count(&db, "credentials"), 0);
    assert_eq!(count(&db, "commit_links"), 2);
    assert_eq!(count(&db, "task_tags"), 2);
    assert!(dir.join("files/notes/30.md").is_file());
    assert!(dir.join("files/attachments/40-spec.pdf").is_file());
}

#[test]
fn round_trip_preserves_everything_under_new_ids() {
    let mut f = fixture("rt");
    let dir = export(&f);
    let report = import_project(&mut f.conn, &f.paths, &dir).unwrap();

    assert_ne!(report.project_id, ProjectId(7));
    assert_eq!(report.name, "Big one (imported)");
    assert_eq!(
        (
            report.tasks,
            report.notes,
            report.attachments,
            report.commit_links
        ),
        (3, 2, 2, 2)
    );
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(
        shape(&f.conn, &f.paths, 7)[1..],
        shape(&f.conn, &f.paths, report.project_id.0)[1..],
        "everything but the project row (name differs) is equivalent"
    );

    // All ids are new and the imported tree is internally consistent.
    let new = report.project_id.0;
    let old_ids_reused: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE project_id = ?1 AND id IN (20, 21, 22)",
            params![new],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old_ids_reused, 0);
    let depth3: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tasks g JOIN tasks c ON c.id = g.parent_id
             JOIN tasks r ON r.id = c.parent_id
             WHERE g.project_id = ?1 AND r.parent_id IS NULL AND g.title = 'grandchild'",
            params![new],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(depth3, 1);
    let violations: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    // The original is untouched and the credential did not travel.
    assert_eq!(count(&f.conn, "credentials"), 1);
    assert_eq!(
        fs::read_to_string(f.paths.notes_dir(ProjectId(7)).join("30.md")).unwrap(),
        "# hello"
    );
    // The copied attachment got a new name derived from its new id.
    let stored: String = f
        .conn
        .query_row(
            "SELECT file_path FROM attachments WHERE project_id = ?1 AND is_linked = 0",
            params![new],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored.ends_with("-spec.pdf") && stored != "40-spec.pdf");
}

#[test]
fn importing_twice_gives_two_independent_projects() {
    let mut f = fixture("twice");
    let dir = export(&f);
    let a = import_project(&mut f.conn, &f.paths, &dir).unwrap();
    let b = import_project(&mut f.conn, &f.paths, &dir).unwrap();
    assert_ne!(a.project_id, b.project_id);
    assert_ne!(a.name, b.name);
    assert_ne!(
        f.paths.project_dir(a.project_id),
        f.paths.project_dir(b.project_id)
    );
    assert_eq!(count(&f.conn, "projects"), 4);
    assert_eq!(count(&f.conn, "tasks"), 1 + 3 + 3 + 3);
    // Deleting one copy (cascade) leaves the other whole.
    f.conn
        .execute(
            "DELETE FROM projects WHERE id = ?1",
            params![a.project_id.0],
        )
        .unwrap();
    let left: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE project_id = ?1",
            params![b.project_id.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 3);
    assert_eq!(
        shape(&f.conn, &f.paths, 7)[1..],
        shape(&f.conn, &f.paths, b.project_id.0)[1..]
    );
}

/// Runs the import expecting a refusal, checks nothing was left behind
/// (rows or folders) and returns the error text.
fn assert_refused_and_clean(f: &mut Fixture, dir: &Path) -> String {
    let projects = count(&f.conn, "projects");
    let tasks = count(&f.conn, "tasks");
    let folders_before = fs::read_dir(f.paths.root().join("projects"))
        .unwrap()
        .count();
    let err = import_project(&mut f.conn, &f.paths, dir).unwrap_err();
    assert_eq!(count(&f.conn, "projects"), projects, "no project row");
    assert_eq!(count(&f.conn, "tasks"), tasks, "no task rows");
    assert_eq!(
        fs::read_dir(f.paths.root().join("projects"))
            .unwrap()
            .count(),
        folders_before,
        "no stray project folder"
    );
    err.to_string()
}

#[test]
fn hostile_note_paths_are_rejected() {
    for bad in [
        "../../escape.md",
        "..\\..\\escape.md",
        "/etc/passwd",
        "C:\\Windows\\win.ini",
        "C:evil.md",
        "notes/../../x.md",
        "",
    ] {
        let mut f = fixture("hostile-note");
        let dir = export(&f);
        let db = Connection::open(dir.join(EXPORT_DB_NAME)).unwrap();
        db.execute(
            "UPDATE notes SET file_path = ?1 WHERE id = 30",
            params![bad],
        )
        .unwrap();
        drop(db);
        let message = assert_refused_and_clean(&mut f, &dir);
        assert!(message.contains("unsafe file path"), "{bad:?}: {message}");
    }
}

#[test]
fn hostile_attachment_paths_are_rejected() {
    let mut f = fixture("hostile-att");
    let dir = export(&f);
    tamper(
        &dir,
        "UPDATE attachments SET file_path = '../../../secret.txt' WHERE id = 40",
    );
    let message = assert_refused_and_clean(&mut f, &dir);
    assert!(message.contains("unsafe file path"), "{message}");
}

#[cfg(unix)]
#[test]
fn symlinked_payload_files_are_rejected() {
    let mut f = fixture("symlink");
    let dir = export(&f);
    let secret = f.scratch.path().join("secret.txt");
    fs::write(&secret, "top secret").unwrap();
    let link = dir.join("files/notes/30.md");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let message = assert_refused_and_clean(&mut f, &dir);
    assert!(message.contains("link or folder"), "{message}");
}

#[test]
fn newer_schema_versions_are_refused() {
    let mut f = fixture("newer");
    let dir = export(&f);
    tamper(&dir, "INSERT INTO schema_migrations VALUES (999);");
    let message = assert_refused_and_clean(&mut f, &dir);
    assert!(message.contains("newer version"), "{message}");
}

#[test]
fn broken_task_trees_are_refused() {
    let mut f = fixture("cycle");
    let dir = export(&f);
    tamper(&dir, "UPDATE tasks SET parent_id = 22 WHERE id = 20;");
    assert!(assert_refused_and_clean(&mut f, &dir).contains("cycle"));

    let mut f = fixture("dangling");
    let dir = export(&f);
    tamper(&dir, "UPDATE tasks SET parent_id = 999 WHERE id = 21;");
    assert!(assert_refused_and_clean(&mut f, &dir).contains("parent"));
}

#[test]
fn not_an_export_is_refused() {
    let mut f = fixture("empty");
    let dir = f.scratch.path().join("empty-dir");
    fs::create_dir_all(&dir).unwrap();
    let message = assert_refused_and_clean(&mut f, &dir);
    assert!(message.contains("not a project export"), "{message}");

    // A random file named project.db is not a database either.
    fs::write(dir.join(EXPORT_DB_NAME), "garbage garbage garbage").unwrap();
    assert_refused_and_clean(&mut f, &dir);
}

#[test]
fn size_caps_are_enforced_and_leave_nothing_behind() {
    let mut f = fixture("caps");
    let dir = export(&f);
    let tiny = ImportLimits {
        max_total_bytes: 1024,
        max_file_bytes: 4,
    };
    let err = import_project_with_limits(&mut f.conn, &f.paths, &dir, tiny).unwrap_err();
    assert!(err.to_string().contains("larger"), "{err}");
    assert_eq!(count(&f.conn, "projects"), 2);

    let small_total = ImportLimits {
        max_total_bytes: 10,
        max_file_bytes: 1024,
    };
    assert!(import_project_with_limits(&mut f.conn, &f.paths, &dir, small_total).is_err());
    assert_eq!(count(&f.conn, "projects"), 2);
    assert_eq!(
        fs::read_dir(f.paths.root().join("projects"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn copy_stops_at_the_cap_even_if_the_file_grew_after_the_check() {
    let scratch = Scratch::new("grow");
    let from = scratch.path().join("big");
    fs::write(&from, vec![0u8; 100]).unwrap();
    let to = scratch.path().join("copy");
    let limits = ImportLimits {
        max_total_bytes: 50,
        max_file_bytes: 50,
    };
    let copies = [FileCopy {
        from,
        to: to.clone(),
    }];
    assert!(copy_all(&copies, &limits).is_err());
    assert!(!to.exists(), "the oversize partial copy is removed");
}

#[test]
fn bad_rows_are_skipped_not_fatal_and_unc_links_are_dropped() {
    let mut f = fixture("skips");
    let dir = export(&f);
    tamper(
        &dir,
        "INSERT INTO task_tags VALUES (21, 'a,b');
         INSERT INTO attachments VALUES (42, 7, NULL, 'unc', '\\\\evil\\share\\x.pdf', 1, 1);
         UPDATE commit_links SET hash = 'not hex!' WHERE id = 51;
         UPDATE tasks SET status = 'weird', priority = 'odd' WHERE id = 21;",
    );
    let report = import_project(&mut f.conn, &f.paths, &dir).unwrap();
    assert_eq!(report.skipped.len(), 3, "{:?}", report.skipped);
    assert_eq!(report.attachments, 2);
    assert_eq!(report.commit_links, 1);
    let (status, priority): (String, String) = f
        .conn
        .query_row(
            "SELECT status, priority FROM tasks WHERE project_id = ?1 AND title = 'child'",
            params![report.project_id.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((status.as_str(), priority.as_str()), ("todo", "medium"));
}

#[test]
fn a_missing_note_file_imports_as_an_empty_note() {
    let mut f = fixture("missing");
    let dir = export(&f);
    fs::remove_file(dir.join("files/notes/30.md")).unwrap();
    let report = import_project(&mut f.conn, &f.paths, &dir).unwrap();
    assert_eq!(report.notes, 2);
    assert_eq!(report.skipped.len(), 1);
    let file: String = f
        .conn
        .query_row(
            "SELECT file_path FROM notes WHERE project_id = ?1 AND title = 'project note'",
            params![report.project_id.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.paths.notes_dir(report.project_id).join(file)).unwrap(),
        ""
    );
}

#[test]
fn older_exports_without_optional_tables_still_import() {
    let mut f = fixture("older");
    let dir = export(&f);
    tamper(
        &dir,
        "DROP TABLE task_tags; DROP TABLE run_commands; DELETE FROM schema_migrations WHERE version = 3;",
    );
    let report = import_project(&mut f.conn, &f.paths, &dir).unwrap();
    assert_eq!(report.tasks, 3);
}
