//! Project import from a `.devspace-export` folder (see [`crate::transfer`]).
//!
//! The export is **untrusted input**: anyone can hand over a folder. So the
//! importer never reuses ids, paths or sizes from it. Every row is
//! re-created under fresh ids in one transaction, every file path is
//! validated and re-derived from the new ids, and copying is capped.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use devspace_core::{normalize_tag, sanitize_file_name, CommitLinkStatus, ProjectId};
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};

use crate::migrations::latest_migration_version;
use crate::transfer::{invalid, table_exists, EXPORT_DB_NAME};
use crate::{StorageError, WorkspacePaths};

/// Caps on what one import may copy onto disk. Sizes in the imported
/// database are never trusted; the real file sizes are checked and the copy
/// itself stops at the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportLimits {
    /// Sum of all copied file bytes.
    pub max_total_bytes: u64,
    /// Largest single copied file.
    pub max_file_bytes: u64,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 2 * 1024 * 1024 * 1024,
            max_file_bytes: 1024 * 1024 * 1024,
        }
    }
}

/// What an import created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// The new project's id (never the exported one).
    pub project_id: ProjectId,
    /// The name it was given (a suffix is added if that name was taken).
    pub name: String,
    pub tasks: usize,
    pub notes: usize,
    pub attachments: usize,
    pub commit_links: usize,
    /// Files copied into the new project's folders.
    pub files_copied: usize,
    /// Harmless problems that were skipped (missing file, bad tag, ...).
    pub skipped: Vec<String>,
}

struct TaskRow {
    id: i64,
    parent: Option<i64>,
    title: String,
    status: String,
    priority: String,
    due_at: Option<i64>,
    branch: Option<String>,
    created_at: i64,
    updated_at: i64,
}

struct NoteRow {
    id: i64,
    task: Option<i64>,
    kind: String,
    title: String,
    file_path: String,
    updated_at: i64,
}

struct AttachmentRow {
    id: i64,
    task: Option<i64>,
    name: String,
    file_path: String,
    is_linked: bool,
    created_at: i64,
}

struct LinkRow {
    task: i64,
    repo: i64,
    hash: String,
    patch_id: Option<String>,
    status: String,
    note: Option<String>,
    created_at: i64,
}

/// A file to copy once the rows are in: source inside the export, and the
/// destination inside the new project folder.
struct FileCopy {
    from: PathBuf,
    to: PathBuf,
}

/// Imports with the default [`ImportLimits`]. See [`import_project_with_limits`].
pub fn import_project(
    conn: &mut Connection,
    paths: &WorkspacePaths,
    export_dir: &Path,
) -> Result<ImportReport, StorageError> {
    import_project_with_limits(conn, paths, export_dir, ImportLimits::default())
}

/// Imports the project in `export_dir` as a NEW project: all ids remapped,
/// one transaction, files copied under names derived from the new ids.
/// Refuses databases from a newer app version, hostile paths (anything but
/// plain relative names, symlinks, escapes) and oversize payloads. On any
/// error nothing is left behind (no rows, no project folder).
pub fn import_project_with_limits(
    conn: &mut Connection,
    paths: &WorkspacePaths,
    export_dir: &Path,
    limits: ImportLimits,
) -> Result<ImportReport, StorageError> {
    let db_path = export_dir.join(EXPORT_DB_NAME);
    let meta = fs::symlink_metadata(&db_path)
        .map_err(|_| invalid(format!("not a project export: no {EXPORT_DB_NAME} found")))?;
    if !meta.is_file() {
        return Err(invalid(format!("{EXPORT_DB_NAME} is not a regular file")));
    }
    let src = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| invalid(format!("{EXPORT_DB_NAME} is not a readable database")))?;
    check_schema_version(&src)?;

    let (name, description, project_created, project_updated) = read_project(&src)?;
    let tasks = read_tasks(&src)?;
    let task_ids: HashSet<i64> = tasks.iter().map(|t| t.id).collect();
    check_task_tree(&tasks, &task_ids)?;
    let tags = read_tags(&src, &task_ids)?;
    let notes = read_notes(&src, &task_ids)?;
    let attachments = read_attachments(&src, &task_ids)?;
    let repos = read_repos(&src)?;
    let links = read_links(&src)?;
    let run_commands = read_run_commands(&src)?;

    let mut skipped = Vec::new();
    let mut copies_plan: Vec<(bool, i64, PathBuf)> = Vec::new(); // (is_note, old id, source)
    let notes_base = fs::canonicalize(export_dir.join("files").join("notes")).ok();
    let attach_base = fs::canonicalize(export_dir.join("files").join("attachments")).ok();
    let mut total: u64 = 0;
    for note in &notes {
        if let Some(source) = resolve_source(notes_base.as_deref(), &note.file_path)? {
            total = add_size(total, &source, &limits)?;
            copies_plan.push((true, note.id, source));
        } else {
            skipped.push(format!("note \"{}\": file missing", note.title));
        }
    }
    for a in attachments.iter().filter(|a| !a.is_linked) {
        if let Some(source) = resolve_source(attach_base.as_deref(), &a.file_path)? {
            total = add_size(total, &source, &limits)?;
            copies_plan.push((false, a.id, source));
        } else {
            skipped.push(format!("attachment \"{}\": file missing", a.name));
        }
    }
    if total > limits.max_total_bytes {
        return Err(invalid("import is larger than the allowed total size"));
    }

    // From here on we write. Immediate: take the write lock now so the ids
    // we pick cannot be taken by someone else before we commit.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut next_project = max_id(&tx, "projects")? + 1;
    while paths.project_dir(ProjectId(next_project)).exists() {
        next_project += 1;
    }
    let new_project = ProjectId(next_project);
    let final_name = unique_project_name(&tx, &name)?;
    tx.execute(
        "INSERT INTO projects (id, name, description, archived, created_at, updated_at)
         VALUES (?1, ?2, ?3, 0, ?4, ?5)",
        params![
            new_project.0,
            final_name,
            description,
            project_created,
            project_updated
        ],
    )?;

    let mut repo_map: HashMap<i64, i64> = HashMap::new();
    let mut next_repo = max_id(&tx, "repos")? + 1;
    for (old, path, name) in &repos {
        tx.execute(
            "INSERT INTO repos (id, project_id, path, name) VALUES (?1, ?2, ?3, ?4)",
            params![next_repo, new_project.0, path, name],
        )?;
        repo_map.insert(*old, next_repo);
        next_repo += 1;
    }

    // New ids in old-id order; insertion parents-first to satisfy the FK.
    let mut task_map: HashMap<i64, i64> = HashMap::new();
    let mut next_task = max_id(&tx, "tasks")? + 1;
    let mut by_old_id: Vec<&TaskRow> = tasks.iter().collect();
    by_old_id.sort_by_key(|t| t.id);
    for t in &by_old_id {
        task_map.insert(t.id, next_task);
        next_task += 1;
    }
    let mut by_depth: Vec<(usize, &TaskRow)> = tasks
        .iter()
        .map(|t| (depth_of(t.id, &tasks), t))
        .collect::<Vec<_>>();
    by_depth.sort_by_key(|(depth, t)| (*depth, t.id));
    for (_, t) in &by_depth {
        tx.execute(
            "INSERT INTO tasks (id, project_id, parent_id, title, status, priority, due_at, branch, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task_map[&t.id],
                new_project.0,
                t.parent.map(|p| task_map[&p]),
                t.title,
                known_or(&t.status, &["todo", "in_progress", "blocked", "done"], "todo"),
                known_or(&t.priority, &["low", "medium", "high", "urgent"], "medium"),
                t.due_at,
                t.branch,
                t.created_at,
                t.updated_at,
            ],
        )?;
    }

    for (task, tag) in &tags {
        match normalize_tag(tag) {
            Ok(tag) => {
                tx.execute(
                    "INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
                    params![task_map[task], tag],
                )?;
            }
            Err(_) => skipped.push(format!("tag {tag:?} is not valid")),
        }
    }

    let mut file_copies: Vec<FileCopy> = Vec::new();
    let sources: HashMap<(bool, i64), &PathBuf> = copies_plan
        .iter()
        .map(|(is_note, id, p)| ((*is_note, *id), p))
        .collect();
    let mut next_note = max_id(&tx, "notes")? + 1;
    paths.ensure_dirs(Some(new_project))?;
    // Removes the new folder on any early return (SQL error, failed copy,
    // failed commit) so nothing is left half-imported.
    let mut folder_guard = NewFolderGuard {
        dir: paths.project_dir(new_project),
        keep: false,
    };
    for n in &notes {
        let new_id = next_note;
        next_note += 1;
        // The stored name is derived from the new id, never from the import.
        let file_name = format!("{new_id}.{}", safe_extension(&n.file_path));
        tx.execute(
            "INSERT INTO notes (id, project_id, task_id, kind, title, file_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                new_id,
                new_project.0,
                n.task.map(|t| task_map[&t]),
                known_or(&n.kind, &["note", "script", "sql"], "note"),
                n.title,
                file_name,
                n.updated_at,
            ],
        )?;
        let to = paths.notes_dir(new_project).join(&file_name);
        match sources.get(&(true, n.id)) {
            Some(from) => file_copies.push(FileCopy {
                from: (*from).clone(),
                to,
            }),
            // Keep the row usable: an empty note file, as the app creates.
            None => fs::write(&to, "")?,
        }
    }

    let mut next_attachment = max_id(&tx, "attachments")? + 1;
    let mut attachments_imported = 0;
    for a in &attachments {
        let stored = if a.is_linked {
            if !is_acceptable_link(&a.file_path) {
                skipped.push(format!("attachment \"{}\": link not accepted", a.name));
                continue;
            }
            a.file_path.clone()
        } else {
            match sources.get(&(false, a.id)) {
                Some(_) => format!("{next_attachment}-{}", sanitize_file_name(&a.name)),
                // No file to go with it, so no row either.
                None => continue,
            }
        };
        tx.execute(
            "INSERT INTO attachments (id, project_id, task_id, name, file_path, is_linked, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                next_attachment,
                new_project.0,
                a.task.map(|t| task_map[&t]),
                a.name,
                stored,
                a.is_linked,
                a.created_at,
            ],
        )?;
        if let (false, Some(from)) = (a.is_linked, sources.get(&(false, a.id))) {
            file_copies.push(FileCopy {
                from: (*from).clone(),
                to: paths.attachments_dir(new_project).join(&stored),
            });
        }
        next_attachment += 1;
        attachments_imported += 1;
    }

    let mut next_link = max_id(&tx, "commit_links")? + 1;
    let mut links_imported = 0;
    for l in &links {
        let valid_hash = !l.hash.is_empty()
            && l.hash.len() <= 64
            && l.hash.chars().all(|c| c.is_ascii_hexdigit());
        let (Some(task), Some(repo)) = (task_map.get(&l.task), repo_map.get(&l.repo)) else {
            skipped.push(format!("commit link {}: task or repo missing", l.hash));
            continue;
        };
        if !valid_hash {
            skipped.push("commit link with an invalid hash".to_string());
            continue;
        }
        let status = CommitLinkStatus::parse(&l.status)
            .unwrap_or(CommitLinkStatus::Linked)
            .as_str();
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO commit_links (id, task_id, repo_id, hash, patch_id, status, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                next_link, task, repo, l.hash, l.patch_id, status, l.note, l.created_at
            ],
        )?;
        if inserted == 1 {
            next_link += 1;
            links_imported += 1;
        }
    }

    if let Some(commands) = run_commands {
        let mut next_command = max_id(&tx, "run_commands")? + 1;
        for (name, command, cwd) in commands {
            if name.trim().is_empty() || command.trim().is_empty() {
                skipped.push("run command with an empty name or text".to_string());
                continue;
            }
            tx.execute(
                "INSERT INTO run_commands (id, project_id, name, command, cwd)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![next_command, new_project.0, name, command, cwd],
            )?;
            next_command += 1;
        }
    }

    // Files last: if a copy fails the transaction is dropped (rolled back)
    // and the new folder removed, so there is nothing half-imported.
    let files_copied = copy_all(&file_copies, &limits)?;
    tx.commit()?;
    folder_guard.keep = true;

    Ok(ImportReport {
        project_id: new_project,
        name: final_name,
        tasks: tasks.len(),
        notes: notes.len(),
        attachments: attachments_imported,
        commit_links: links_imported,
        files_copied,
        skipped,
    })
}

/// Deletes a freshly created project folder when dropped, unless `keep`.
struct NewFolderGuard {
    dir: PathBuf,
    keep: bool,
}

impl Drop for NewFolderGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

fn check_schema_version(src: &Connection) -> Result<(), StorageError> {
    if !table_exists(src, "schema_migrations")? {
        return Err(invalid("not a Devspace export (no schema version)"));
    }
    let newest: Option<i64> =
        src.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })?;
    match newest {
        None => Err(invalid("not a Devspace export (empty schema version)")),
        Some(v) if v > latest_migration_version() => Err(invalid(format!(
            "this export was made by a newer version of Devspace (schema {v}, this app knows {})",
            latest_migration_version()
        ))),
        Some(_) => Ok(()),
    }
}

fn read_project(src: &Connection) -> Result<(String, Option<String>, i64, i64), StorageError> {
    let mut stmt = src.prepare("SELECT name, description, created_at, updated_at FROM projects")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<Vec<(String, Option<String>, i64, i64)>, _>>()?;
    let [(name, description, created, updated)] = <[_; 1]>::try_from(rows)
        .map_err(|_| invalid("an export must contain exactly one project"))?;
    let name = if name.trim().is_empty() {
        "Imported project".to_string()
    } else {
        name
    };
    Ok((name, description, created, updated))
}

fn read_tasks(src: &Connection) -> Result<Vec<TaskRow>, StorageError> {
    let mut stmt = src.prepare(
        "SELECT id, parent_id, title, status, priority, due_at, branch, created_at, updated_at
         FROM tasks ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TaskRow {
            id: r.get(0)?,
            parent: r.get(1)?,
            title: r.get(2)?,
            status: r.get(3)?,
            priority: r.get(4)?,
            due_at: r.get(5)?,
            branch: r.get(6)?,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
        })
    })?;
    let mut tasks = rows.collect::<Result<Vec<_>, _>>()?;
    for t in &mut tasks {
        if t.title.trim().is_empty() {
            t.title = "(untitled)".to_string();
        }
    }
    Ok(tasks)
}

/// Every parent must exist in the export and the tree must be acyclic.
fn check_task_tree(tasks: &[TaskRow], ids: &HashSet<i64>) -> Result<(), StorageError> {
    let parents: HashMap<i64, Option<i64>> = tasks.iter().map(|t| (t.id, t.parent)).collect();
    for t in tasks {
        if t.parent.is_some_and(|p| !ids.contains(&p)) {
            return Err(invalid("a task points at a parent that is not in the export"));
        }
        let mut current = t.parent;
        let mut steps = 0;
        while let Some(p) = current {
            steps += 1;
            if p == t.id || steps > tasks.len() {
                return Err(invalid("the task tree contains a cycle"));
            }
            current = parents.get(&p).copied().flatten();
        }
    }
    Ok(())
}

/// Depth of a task (0 = top level). Callers have already checked the tree.
fn depth_of(id: i64, tasks: &[TaskRow]) -> usize {
    let parents: HashMap<i64, Option<i64>> = tasks.iter().map(|t| (t.id, t.parent)).collect();
    let mut depth = 0;
    let mut current = parents.get(&id).copied().flatten();
    while let Some(p) = current {
        depth += 1;
        current = parents.get(&p).copied().flatten();
    }
    depth
}

fn read_tags(src: &Connection, tasks: &HashSet<i64>) -> Result<Vec<(i64, String)>, StorageError> {
    if !table_exists(src, "task_tags")? {
        return Ok(Vec::new());
    }
    let mut stmt = src.prepare("SELECT task_id, tag FROM task_tags ORDER BY task_id, tag")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let all = rows.collect::<Result<Vec<_>, _>>()?;
    if all.iter().any(|(t, _)| !tasks.contains(t)) {
        return Err(invalid("a tag belongs to a task that is not in the export"));
    }
    Ok(all)
}

fn read_notes(src: &Connection, tasks: &HashSet<i64>) -> Result<Vec<NoteRow>, StorageError> {
    let mut stmt =
        src.prepare("SELECT id, task_id, kind, title, file_path, updated_at FROM notes ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(NoteRow {
            id: r.get(0)?,
            task: r.get(1)?,
            kind: r.get(2)?,
            title: r.get(3)?,
            file_path: r.get(4)?,
            updated_at: r.get(5)?,
        })
    })?;
    let mut notes = rows.collect::<Result<Vec<_>, _>>()?;
    for n in &mut notes {
        if n.task.is_some_and(|t| !tasks.contains(&t)) {
            return Err(invalid("a note belongs to a task that is not in the export"));
        }
        if n.title.trim().is_empty() {
            n.title = "(untitled)".to_string();
        }
    }
    Ok(notes)
}

fn read_attachments(
    src: &Connection,
    tasks: &HashSet<i64>,
) -> Result<Vec<AttachmentRow>, StorageError> {
    let mut stmt = src.prepare(
        "SELECT id, task_id, name, file_path, is_linked, created_at FROM attachments ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(AttachmentRow {
            id: r.get(0)?,
            task: r.get(1)?,
            name: r.get(2)?,
            file_path: r.get(3)?,
            is_linked: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    let attachments = rows.collect::<Result<Vec<_>, _>>()?;
    if attachments
        .iter()
        .any(|a| a.task.is_some_and(|t| !tasks.contains(&t)))
    {
        return Err(invalid(
            "an attachment belongs to a task that is not in the export",
        ));
    }
    Ok(attachments)
}

fn read_repos(src: &Connection) -> Result<Vec<(i64, String, String)>, StorageError> {
    let mut stmt = src.prepare("SELECT id, path, name FROM repos ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn read_links(src: &Connection) -> Result<Vec<LinkRow>, StorageError> {
    let mut stmt = src.prepare(
        "SELECT task_id, repo_id, hash, patch_id, status, note, created_at
         FROM commit_links ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(LinkRow {
            task: r.get(0)?,
            repo: r.get(1)?,
            hash: r.get(2)?,
            patch_id: r.get(3)?,
            status: r.get(4)?,
            note: r.get(5)?,
            created_at: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

type RunCommandRow = (String, String, Option<String>);

fn read_run_commands(src: &Connection) -> Result<Option<Vec<RunCommandRow>>, StorageError> {
    if !table_exists(src, "run_commands")? {
        return Ok(None);
    }
    let mut stmt = src.prepare("SELECT name, command, cwd FROM run_commands ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(Some(rows.collect::<Result<Vec<_>, _>>()?))
}

fn max_id(conn: &Connection, table: &str) -> Result<i64, StorageError> {
    // `table` is always one of our own constant table names.
    let max: Option<i64> =
        conn.query_row(&format!("SELECT MAX(id) FROM {table}"), [], |r| r.get(0))?;
    Ok(max.unwrap_or(0))
}

fn unique_project_name(conn: &Connection, name: &str) -> Result<String, StorageError> {
    let taken = |candidate: &str| -> Result<bool, StorageError> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE name = ?1)",
            params![candidate],
            |r| r.get(0),
        )?)
    };
    if !taken(name)? {
        return Ok(name.to_string());
    }
    let first = format!("{name} (imported)");
    if !taken(&first)? {
        return Ok(first);
    }
    for n in 2..10_000 {
        let candidate = format!("{name} (imported {n})");
        if !taken(&candidate)? {
            return Ok(candidate);
        }
    }
    Ok(first)
}

fn known_or<'a>(value: &'a str, known: &[&str], default: &'a str) -> &'a str {
    if known.contains(&value) {
        value
    } else {
        default
    }
}

/// A file extension safe to reuse in a stored note name: short and
/// alphanumeric, otherwise `md`.
fn safe_extension(file_path: &str) -> String {
    Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "md".to_string())
}

/// A linked attachment is only a reference to a file on this machine, but
/// the path still comes from the import: refuse network (UNC) paths, which
/// would make the OS contact a remote host when the link is opened.
fn is_acceptable_link(path: &str) -> bool {
    !path.trim().is_empty()
        && !path.contains('\0')
        && !path.starts_with("\\\\")
        && !path.starts_with("//")
}

/// Locates `rel` inside `base` (the canonical `files/<kind>` folder) or
/// returns `None` if the file just isn't there. Anything suspicious (not a
/// plain relative name, a symlink, a directory, resolving outside `base`) is
/// an error: a well-formed export never contains it.
fn resolve_source(base: Option<&Path>, rel: &str) -> Result<Option<PathBuf>, StorageError> {
    let only_plain_components = Path::new(rel)
        .components()
        .all(|c| matches!(c, Component::Normal(_)));
    if rel.is_empty()
        || rel.contains('\0')
        || !devspace_core::is_safe_relative_path(rel)
        || !only_plain_components
    {
        return Err(invalid(format!("unsafe file path in import: {rel:?}")));
    }
    let Some(base) = base else {
        return Ok(None);
    };
    let candidate = base.join(rel);
    let meta = match fs::symlink_metadata(&candidate) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_file() {
        return Err(invalid(format!(
            "import file is a link or folder, not a file: {rel:?}"
        )));
    }
    let canonical = fs::canonicalize(&candidate)?;
    if !canonical.starts_with(base) {
        return Err(invalid(format!("import file escapes the export: {rel:?}")));
    }
    Ok(Some(canonical))
}

fn add_size(total: u64, file: &Path, limits: &ImportLimits) -> Result<u64, StorageError> {
    let len = fs::metadata(file)?.len();
    if len > limits.max_file_bytes {
        return Err(invalid("an imported file is larger than the allowed size"));
    }
    Ok(total.saturating_add(len))
}

/// Copies the planned files, never overwriting, and enforcing the byte caps
/// on what is actually read (not on sizes reported earlier).
fn copy_all(copies: &[FileCopy], limits: &ImportLimits) -> Result<usize, StorageError> {
    let mut remaining = limits.max_total_bytes;
    for copy in copies {
        let cap = remaining.min(limits.max_file_bytes);
        let mut input = fs::File::open(&copy.from)?.take(cap.saturating_add(1));
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&copy.to)?;
        let written = std::io::copy(&mut input, &mut output)?;
        if written > cap {
            drop(output);
            let _ = fs::remove_file(&copy.to);
            return Err(invalid("imported files exceed the allowed size"));
        }
        remaining -= written;
    }
    Ok(copies.len())
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
