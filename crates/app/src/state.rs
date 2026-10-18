//! App-wide state: the sqlite connection and current selection. Lives in a
//! GPUI entity so views update via `cx.notify()` rather than holding state
//! themselves (CLAUDE.md GPUI guidelines).

use devspace_core::{
    descendants_of, sanitize_file_name, Attachment, AttachmentId, CommitLink, CommitLinkId,
    CommitLinkStatus, Note, NoteId, NoteKind, Project, ProjectId, Repo, RepoId, Task, TaskId,
    TaskPriority, TaskStatus,
};
use devspace_storage::{
    AttachmentRepo, CommitLinkRepo, NoteRepo, ProjectRepo, RepoRepo, TaskRepo, WorkspacePaths,
};
use rusqlite::Connection;

mod run_commands; // run-command helpers (state/run_commands.rs)
use std::time::{SystemTime, UNIX_EPOCH};

/// How many commits the dashboard's "recent commits" section shows. Paged
/// rather than loading full history (CLAUDE.md performance rule).
const RECENT_COMMITS_LIMIT: usize = 5;

/// A prepared attachment: the row to insert and, for copied files, the
/// source/destination pair. Splitting preparation from execution lets the
/// (possibly large) file copy run on a background task instead of the UI
/// thread (CLAUDE.md GPUI guidelines).
pub struct AttachmentJob {
    attachment: Attachment,
    copy: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

impl AttachmentJob {
    /// Streams the file into the workspace. Never overwrites: the
    /// destination name carries the attachment id, so it can't already
    /// exist; if it does something is wrong and we stop. A partial copy is
    /// removed on failure.
    pub fn run(&self) -> std::io::Result<()> {
        let Some((from, to)) = &self.copy else {
            return Ok(());
        };
        let mut input = std::fs::File::open(from)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        if let Err(e) = std::io::copy(&mut input, &mut output) {
            drop(output);
            let _ = std::fs::remove_file(to);
            return Err(e);
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.attachment.name
    }
}

pub struct AppState {
    conn: Connection,
    paths: WorkspacePaths,
    pub projects: Vec<Project>,
    pub selected_project: Option<ProjectId>,
    pub tasks: Vec<Task>,
    pub notes: Vec<Note>,
    pub repos: Vec<Repo>,
    /// Recent commits across the selected project's linked repos, newest
    /// first. Recomputed on `select_project`/repo link/unlink rather than
    /// per-render, since each refresh opens the repo and walks its history.
    pub recent_commits: Vec<devspace_git::RepoCommit>,
    /// Names of linked repos that could not be read on the last refresh
    /// (moved, deleted, not a git repo), so the dashboard can say so rather
    /// than silently showing a partial history.
    pub unreadable_repos: Vec<String>,
    /// Commit links of the selected project (every task, any depth), for
    /// the commit browser's badges and the unassigned filter.
    pub commit_links: Vec<CommitLink>,
    next_commit_link_id: i64,
    /// Attachments of the selected project (project-level and per-task).
    pub attachments: Vec<Attachment>,
    next_attachment_id: i64,
    /// Saved run commands of the selected project (`state/run_commands.rs`).
    pub run_commands: Vec<devspace_core::RunCommand>,
    next_run_command_id: i64,
    next_project_id: i64,
    next_task_id: i64,
    next_note_id: i64,
    next_repo_id: i64,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl AppState {
    pub fn new(conn: Connection, paths: WorkspacePaths) -> Self {
        let mut state = Self {
            conn,
            paths,
            projects: Vec::new(),
            selected_project: None,
            tasks: Vec::new(),
            notes: Vec::new(),
            repos: Vec::new(),
            recent_commits: Vec::new(),
            unreadable_repos: Vec::new(),
            commit_links: Vec::new(),
            next_commit_link_id: 1,
            attachments: Vec::new(),
            next_attachment_id: 1,
            run_commands: Vec::new(),
            next_run_command_id: 1,
            next_project_id: 1,
            next_task_id: 1,
            next_note_id: 1,
            next_repo_id: 1,
        };
        state.reload_projects();
        if let Some(first) = state.projects.first() {
            state.select_project(first.id);
        }
        state
    }

    fn next_id(counter: &mut i64, existing_max: i64) -> i64 {
        if existing_max >= *counter {
            *counter = existing_max + 1;
        }
        let id = *counter;
        *counter += 1;
        id
    }

    /// On-disk layout, for background tasks that open their own read-only
    /// connection (the palette's cross-project search).
    pub fn workspace_paths(&self) -> &WorkspacePaths {
        &self.paths
    }

    pub fn reload_projects(&mut self) {
        self.projects = ProjectRepo::list(&self.conn, false).unwrap_or_default();
        if let Some(max) = self.projects.iter().map(|p| p.id.0).max() {
            self.next_project_id = self.next_project_id.max(max + 1);
        }
    }

    pub fn create_project(&mut self, name: &str) {
        let id = ProjectId(Self::next_id(
            &mut self.next_project_id,
            self.projects.iter().map(|p| p.id.0).max().unwrap_or(0),
        ));
        let Ok(project) = Project::new(id, name, now()) else {
            return;
        };
        if ProjectRepo::insert(&self.conn, &project).is_ok() {
            self.reload_projects();
            self.select_project(project.id);
        }
    }

    pub fn select_project(&mut self, id: ProjectId) {
        self.selected_project = Some(id);
        self.tasks = TaskRepo::list_for_project(&self.conn, id).unwrap_or_default();
        if let Some(max) = self.tasks.iter().map(|t| t.id.0).max() {
            self.next_task_id = self.next_task_id.max(max + 1);
        }
        self.notes = NoteRepo::list_for_project(&self.conn, id).unwrap_or_default();
        if let Some(max) = self.notes.iter().map(|n| n.id.0).max() {
            self.next_note_id = self.next_note_id.max(max + 1);
        }
        self.repos = RepoRepo::list_for_project(&self.conn, id).unwrap_or_default();
        if let Some(max) = self.repos.iter().map(|r| r.id.0).max() {
            self.next_repo_id = self.next_repo_id.max(max + 1);
        }
        self.reload_recent_commits();
        self.reload_commit_links();
        self.reload_attachments();
        self.reload_run_commands();
    }

    fn reload_commit_links(&mut self) {
        let Some(project) = self.selected_project else {
            self.commit_links.clear();
            return;
        };
        self.commit_links =
            CommitLinkRepo::list_for_project(&self.conn, project).unwrap_or_default();
        if let Some(max) = self.commit_links.iter().map(|l| l.id.0).max() {
            self.next_commit_link_id = self.next_commit_link_id.max(max + 1);
        }
    }

    /// Maps commits (identified by `(repo, hash)`) to a task at any depth of
    /// the selected project's tree. Commits already linked to that task are
    /// skipped. Returns how many new links were made, or a message fit for
    /// the UI (e.g. the cross-project rule firing).
    pub fn link_commits(
        &mut self,
        task_id: TaskId,
        commits: &[(RepoId, String)],
        status: CommitLinkStatus,
        note: Option<String>,
    ) -> Result<usize, String> {
        let created_at = now();
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let mut links = Vec::with_capacity(commits.len());
        for (repo_id, hash) in commits {
            let id = CommitLinkId(Self::next_id(
                &mut self.next_commit_link_id,
                self.commit_links.iter().map(|l| l.id.0).max().unwrap_or(0),
            ));
            let mut link = CommitLink::new(id, task_id, *repo_id, hash.clone(), created_at)
                .map_err(|e| e.to_string())?;
            link.status = status;
            link.note = note.clone();
            links.push(link);
        }
        let created =
            CommitLinkRepo::insert_skipping_existing(&self.conn, &links).map_err(|e| e.to_string());
        self.reload_commit_links();
        created
    }

    fn reload_attachments(&mut self) {
        let Some(project) = self.selected_project else {
            self.attachments.clear();
            return;
        };
        self.attachments =
            AttachmentRepo::list_for_project(&self.conn, project).unwrap_or_default();
        if let Some(max) = self.attachments.iter().map(|a| a.id.0).max() {
            self.next_attachment_id = self.next_attachment_id.max(max + 1);
        }
    }

    pub fn attachments_for_task(&self, task: TaskId) -> Vec<Attachment> {
        self.attachments
            .iter()
            .filter(|a| a.task_id == Some(task))
            .cloned()
            .collect()
    }

    /// Step one of attaching a file to a task/subtask (or the project when
    /// `task` is `None`): validates the source and allocates the stored
    /// name. `copy` copies the file into the project's attachments folder;
    /// otherwise the attachment links to the original path. Run the returned
    /// job (off the UI thread), then pass it to [`Self::finish_attachment`].
    pub fn prepare_attachment(
        &mut self,
        task: Option<TaskId>,
        source: &std::path::Path,
        copy: bool,
    ) -> Result<AttachmentJob, String> {
        let project = self.selected_project.ok_or("No project selected")?;
        if let Some(task_id) = task {
            if !self.tasks.iter().any(|t| t.id == task_id) {
                return Err("Task not found in this project".into());
            }
        }
        if !source.is_file() {
            return Err("That is not a file".into());
        }
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or("File has no name")?;
        let id = AttachmentId(Self::next_id(
            &mut self.next_attachment_id,
            self.attachments.iter().map(|a| a.id.0).max().unwrap_or(0),
        ));

        let (stored_path, job_copy) = if copy {
            self.paths
                .ensure_dirs(Some(project))
                .map_err(|e| e.to_string())?;
            // The id prefix keeps two files with the same name apart.
            let stored = format!("{}-{}", id.0, sanitize_file_name(&name));
            let dest = self.paths.attachments_dir(project).join(&stored);
            (stored, Some((source.to_path_buf(), dest)))
        } else {
            (source.to_string_lossy().to_string(), None)
        };
        let mut attachment = Attachment::new(id, project, name, stored_path, !copy, now())
            .map_err(|e| e.to_string())?;
        attachment.task_id = task;
        Ok(AttachmentJob {
            attachment,
            copy: job_copy,
        })
    }

    /// Step two: records the attachment once its file (if any) is in place.
    /// If the row can't be saved, the copied file is removed again.
    pub fn finish_attachment(&mut self, job: AttachmentJob) -> Result<(), String> {
        match AttachmentRepo::insert(&self.conn, &job.attachment) {
            Ok(()) => {
                self.attachments.push(job.attachment);
                Ok(())
            }
            Err(e) => {
                if let Some((_, dest)) = &job.copy {
                    let _ = std::fs::remove_file(dest);
                }
                Err(e.to_string())
            }
        }
    }

    /// The file an attachment points at, or `None` if it can't be trusted.
    /// Copied attachments must resolve (symlinks included) to somewhere
    /// inside the project's attachments folder; linked ones are the user's
    /// own path and are returned as stored.
    pub fn attachment_path(&self, attachment: &Attachment) -> Option<std::path::PathBuf> {
        if attachment.is_linked {
            return Some(std::path::PathBuf::from(&attachment.file_path));
        }
        let base = std::fs::canonicalize(self.paths.attachments_dir(attachment.project_id)).ok()?;
        let full = std::fs::canonicalize(base.join(&attachment.file_path)).ok()?;
        full.starts_with(&base).then_some(full)
    }

    /// Removes an attachment. A copied file is deleted from the workspace
    /// (only if it verifiably lives there); a linked file is never touched.
    pub fn remove_attachment(&mut self, id: AttachmentId) {
        let Some(attachment) = self.attachments.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if !attachment.is_linked {
            if let Some(path) = self.attachment_path(&attachment) {
                let _ = std::fs::remove_file(path);
            }
        }
        if AttachmentRepo::delete(&self.conn, id).is_ok() {
            self.attachments.retain(|a| a.id != id);
        }
    }

    /// Deletes the files that belong to `tasks` (copied attachments and note
    /// files). The database cascades their rows when a task is deleted but
    /// knows nothing about the files, which would otherwise be orphaned.
    fn remove_task_files(&self, tasks: &[TaskId]) {
        let Some(project) = self.selected_project else {
            return;
        };
        for attachment in self
            .attachments
            .iter()
            .filter(|a| !a.is_linked && a.task_id.is_some_and(|t| tasks.contains(&t)))
        {
            if let Some(path) = self.attachment_path(attachment) {
                let _ = std::fs::remove_file(path);
            }
        }
        for note in self
            .notes
            .iter()
            .filter(|n| n.task_id.is_some_and(|t| tasks.contains(&t)))
        {
            let _ = std::fs::remove_file(self.paths.notes_dir(project).join(&note.file_path));
        }
    }

    pub fn unlink_commit(&mut self, id: CommitLinkId) {
        if CommitLinkRepo::delete(&self.conn, id).is_ok() {
            self.commit_links.retain(|l| l.id != id);
        }
    }

    /// Advances a link's label: Linked -> Needs migration -> Cherry-picked
    /// -> Deployed -> Linked. (Missing is set by rebase detection, and
    /// cycles back to Linked if the user clears it.)
    pub fn cycle_link_status(&mut self, id: CommitLinkId) {
        let Some(link) = self.commit_links.iter_mut().find(|l| l.id == id) else {
            return;
        };
        link.status = match link.status {
            CommitLinkStatus::Linked => CommitLinkStatus::NeedsMigration,
            CommitLinkStatus::NeedsMigration => CommitLinkStatus::CherryPicked,
            CommitLinkStatus::CherryPicked => CommitLinkStatus::Deployed,
            CommitLinkStatus::Deployed | CommitLinkStatus::Missing => CommitLinkStatus::Linked,
        };
        let _ = CommitLinkRepo::update_status_and_note(
            &self.conn,
            link.id,
            link.status,
            link.note.as_deref(),
        );
    }

    /// Re-walks every linked repo's history, merged newest-first, for the
    /// dashboard's "recent commits" section. Runs on the calling thread;
    /// with a small page size that's cheap, but the full commit browser
    /// must page this on a background task (CLAUDE.md GPUI guidelines).
    fn reload_recent_commits(&mut self) {
        let page = devspace_git::merged_history(&self.repos, RECENT_COMMITS_LIMIT, None);
        self.unreadable_repos = page
            .unreadable
            .iter()
            .filter_map(|id| self.repos.iter().find(|r| r.id == *id))
            .map(|r| r.name.clone())
            .collect();
        self.recent_commits = page.commits;
    }

    /// Links a git repo to the currently selected project. `name` is
    /// whatever label the user gave it (defaults to the last path
    /// component in the UI); the path itself is never validated as an
    /// actual git repo here — `recent_commits` simply comes back empty if
    /// it isn't one, same as a repo with no commits yet.
    pub fn link_repo(&mut self, path: &str, name: &str) {
        let Some(project_id) = self.selected_project else {
            return;
        };
        let id = RepoId(Self::next_id(
            &mut self.next_repo_id,
            self.repos.iter().map(|r| r.id.0).max().unwrap_or(0),
        ));
        let Ok(repo) = Repo::new(id, project_id, path, name) else {
            return;
        };
        if RepoRepo::insert(&self.conn, &repo).is_ok() {
            self.select_project(project_id);
        }
    }

    pub fn unlink_repo(&mut self, id: RepoId) {
        if RepoRepo::delete(&self.conn, id).is_ok() {
            self.repos.retain(|r| r.id != id);
            self.reload_recent_commits();
            self.reload_commit_links();
        }
    }

    /// Creates a project-level markdown note: writes an empty file under
    /// `<project>/notes/` and inserts the db row pointing at it.
    pub fn create_note(&mut self, title: &str) {
        self.create_note_for(None, title);
    }

    /// Creates a markdown note on the project (`task == None`) or on a task
    /// or subtask at any depth. The task must belong to the selected
    /// project. Returns the new note's id.
    pub fn create_note_for(&mut self, task: Option<TaskId>, title: &str) -> Option<NoteId> {
        let project_id = self.selected_project?;
        if let Some(task_id) = task {
            if !self.tasks.iter().any(|t| t.id == task_id) {
                return None;
            }
        }
        let id = NoteId(Self::next_id(
            &mut self.next_note_id,
            self.notes.iter().map(|n| n.id.0).max().unwrap_or(0),
        ));
        let file_name = format!("{}.md", id.0);
        let mut note = Note::new(id, project_id, NoteKind::Note, title, &file_name, now()).ok()?;
        note.task_id = task;
        self.paths.ensure_dirs(Some(project_id)).ok()?;
        let full_path = self.paths.notes_dir(project_id).join(&file_name);
        std::fs::write(&full_path, "").ok()?;
        if NoteRepo::insert(&self.conn, &note).is_err() {
            let _ = std::fs::remove_file(&full_path);
            return None;
        }
        // Newest first, matching how `select_project` orders them.
        self.notes.insert(0, note);
        Some(id)
    }

    /// Notes attached to one task or subtask, newest first.
    pub fn notes_for_task(&self, task: TaskId) -> Vec<Note> {
        self.notes
            .iter()
            .filter(|n| n.task_id == Some(task))
            .cloned()
            .collect()
    }

    /// Project-level notes only (not those belonging to a task).
    pub fn project_notes(&self) -> Vec<Note> {
        self.notes
            .iter()
            .filter(|n| n.task_id.is_none())
            .cloned()
            .collect()
    }

    pub fn rename_note(&mut self, id: NoteId, title: &str) {
        let Some(note) = self.notes.iter_mut().find(|n| n.id == id) else {
            return;
        };
        note.title = title.to_string();
        note.updated_at = now();
        let _ = NoteRepo::update(&self.conn, note);
    }

    pub fn delete_note(&mut self, id: NoteId) {
        let Some(note) = self.notes.iter().find(|n| n.id == id) else {
            return;
        };
        let Some(project_id) = self.selected_project else {
            return;
        };
        let full_path = self.paths.notes_dir(project_id).join(&note.file_path);
        let _ = std::fs::remove_file(full_path);
        if NoteRepo::delete(&self.conn, id).is_ok() {
            self.notes.retain(|n| n.id != id);
        }
    }

    pub fn read_note_content(&self, id: NoteId) -> Option<String> {
        let note = self.notes.iter().find(|n| n.id == id)?;
        let project_id = self.selected_project?;
        let full_path = self.paths.notes_dir(project_id).join(&note.file_path);
        std::fs::read_to_string(full_path).ok()
    }

    pub fn write_note_content(&mut self, id: NoteId, content: &str) {
        let Some(note) = self.notes.iter_mut().find(|n| n.id == id) else {
            return;
        };
        let Some(project_id) = self.selected_project else {
            return;
        };
        let full_path = self.paths.notes_dir(project_id).join(&note.file_path);
        if std::fs::write(full_path, content).is_err() {
            return;
        }
        note.updated_at = now();
        let _ = NoteRepo::update(&self.conn, note);
    }

    pub fn create_task(&mut self, title: &str) {
        let Some(project_id) = self.selected_project else {
            return;
        };
        let id = TaskId(Self::next_id(
            &mut self.next_task_id,
            self.tasks.iter().map(|t| t.id.0).max().unwrap_or(0),
        ));
        let Ok(task) = Task::new(id, project_id, title, now()) else {
            return;
        };
        if TaskRepo::insert(&self.conn, &task).is_ok() {
            self.select_project(project_id);
        }
    }

    pub fn cycle_task_status(&mut self, id: TaskId) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let next = match task.status {
            TaskStatus::Todo => TaskStatus::InProgress,
            TaskStatus::InProgress => TaskStatus::Done,
            TaskStatus::Done => TaskStatus::Todo,
            TaskStatus::Blocked => TaskStatus::Todo,
        };
        task.set_status(next, now());
        let _ = TaskRepo::update(&self.conn, task);
    }

    /// Cycles Low -> Medium -> High -> Urgent -> Low.
    pub fn cycle_task_priority(&mut self, id: TaskId) {
        let next = |p| match p {
            TaskPriority::Low => TaskPriority::Medium,
            TaskPriority::Medium => TaskPriority::High,
            TaskPriority::High => TaskPriority::Urgent,
            TaskPriority::Urgent => TaskPriority::Low,
        };
        self.update_task(id, |task| task.priority = next(task.priority));
    }

    /// Sets or clears (`None`) a task's due date (unix seconds, UTC midnight).
    pub fn set_task_due(&mut self, id: TaskId, due_at: Option<i64>) {
        self.update_task(id, |task| task.due_at = due_at);
    }

    /// Sets or clears the git branch a task is worked on. Blank clears it.
    pub fn set_task_branch(&mut self, id: TaskId, branch: Option<String>) {
        let branch = branch
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty());
        self.update_task(id, |task| task.branch = branch);
    }

    /// Adds a tag to a task. Returns `Ok(false)` if it already had it
    /// (ignoring case), or a message fit for the UI if the tag is invalid.
    pub fn add_task_tag(&mut self, id: TaskId, tag: &str) -> Result<bool, String> {
        if !self.tasks.iter().any(|t| t.id == id) {
            return Err("Task not found".into());
        }
        let added = TaskRepo::add_tag(&self.conn, id, tag).map_err(|e| e.to_string())?;
        if added {
            // The repository already validated the tag; mirror it in memory.
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                let _ = task.add_tag(tag, now());
            }
        }
        Ok(added)
    }

    pub fn remove_task_tag(&mut self, id: TaskId, tag: &str) {
        if TaskRepo::remove_tag(&self.conn, id, tag).is_ok() {
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                task.remove_tag(tag, now());
            }
        }
    }

    /// Every tag used in the selected project with how many tasks carry it,
    /// grouped ignoring case (first spelling wins) and sorted by name — the
    /// options for the project page's tag filter.
    pub fn project_tags(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for tag in self.tasks.iter().flat_map(|t| t.tags.iter()) {
            let lower = tag.to_lowercase();
            match counts
                .iter_mut()
                .find(|(name, _)| name.to_lowercase() == lower)
            {
                Some((_, n)) => *n += 1,
                None => counts.push((tag.clone(), 1)),
            }
        }
        counts.sort_by_key(|(name, _)| name.to_lowercase());
        counts
    }

    fn update_task(&mut self, id: TaskId, change: impl FnOnce(&mut Task)) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        change(task);
        task.updated_at = now();
        let _ = TaskRepo::update(&self.conn, task);
    }

    pub fn rename_project(&mut self, id: ProjectId, name: &str) {
        let Some(project) = self.projects.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if project.rename(name, now()).is_ok() {
            let _ = ProjectRepo::update(&self.conn, project);
        }
    }

    pub fn archive_project(&mut self, id: ProjectId) {
        let Some(project) = self.projects.iter_mut().find(|p| p.id == id) else {
            return;
        };
        project.archive(now());
        let _ = ProjectRepo::update(&self.conn, project);
        let was_selected = self.selected_project == Some(id);
        self.reload_projects();
        if was_selected {
            self.selected_project = None;
            self.tasks.clear();
            if let Some(first) = self.projects.first() {
                self.select_project(first.id);
            }
        }
    }

    pub fn rename_task(&mut self, id: TaskId, title: &str) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let now = now();
        task.title = title.to_string();
        task.updated_at = now;
        let _ = TaskRepo::update(&self.conn, task);
    }

    pub fn delete_task(&mut self, id: TaskId) {
        let mut doomed = descendants_of(&self.tasks, id);
        doomed.push(id);
        if TaskRepo::delete(&self.conn, id).is_ok() {
            // The database cascades to every descendant (and their commit
            // links, notes and attachment rows). Remove the files those rows
            // pointed at, then reload rather than patching in memory.
            self.remove_task_files(&doomed);
            if let Some(project) = self.selected_project {
                self.tasks = TaskRepo::list_for_project(&self.conn, project).unwrap_or_default();
                self.notes = NoteRepo::list_for_project(&self.conn, project).unwrap_or_default();
            }
            self.reload_commit_links();
            self.reload_attachments();
        }
    }

    /// Creates a task nested under `parent_id` in the currently selected
    /// project. Returns the new task's id so callers can e.g. jump straight
    /// into renaming it.
    pub fn create_subtask(&mut self, parent_id: TaskId, title: &str) -> Option<TaskId> {
        let project_id = self.selected_project?;
        let id = TaskId(Self::next_id(
            &mut self.next_task_id,
            self.tasks.iter().map(|t| t.id.0).max().unwrap_or(0),
        ));
        let mut task = Task::new(id, project_id, title, now()).ok()?;
        task.parent_id = Some(parent_id);
        if TaskRepo::insert(&self.conn, &task).is_ok() {
            self.select_project(project_id);
            Some(id)
        } else {
            None
        }
    }

    /// Where a terminal opened for the selected project should start
    /// (CLAUDE.md: "starting in the project folder"): the first linked
    /// repo's path, or the user's home directory if none is linked yet.
    pub fn terminal_cwd(&self) -> std::path::PathBuf {
        self.repos
            .first()
            .map(|repo| std::path::PathBuf::from(&repo.path))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| self.paths.root().to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devspace_storage::{open_in_memory, run_migrations};
    use std::path::{Path, PathBuf};

    /// A scratch workspace under the OS temp dir, removed on drop. (No
    /// `tempfile` dependency in this crate; this is all the tests need.)
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "devspace-state-test-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// State with project "P" selected and a root task "A" plus subtask "B"
    /// under it, with its data under a scratch directory.
    fn setup(scratch: &Scratch) -> (AppState, TaskId, TaskId) {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        let mut state = AppState::new(conn, WorkspacePaths::new(scratch.path().join("data")));
        state.create_project("P");
        state.create_task("A");
        let a = state.tasks[0].id;
        let b = state.create_subtask(a, "B").unwrap();
        (state, a, b)
    }

    fn source_file(scratch: &Scratch, name: &str, content: &str) -> PathBuf {
        let path = scratch.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn attach(state: &mut AppState, task: TaskId, source: &Path, copy: bool) -> AttachmentId {
        let job = state.prepare_attachment(Some(task), source, copy).unwrap();
        job.run().unwrap();
        let id = job.attachment.id;
        state.finish_attachment(job).unwrap();
        id
    }

    #[test]
    fn copied_attachment_lands_in_the_project_folder_and_is_removed_with_its_row() {
        let scratch = Scratch::new();
        let (mut state, _a, b) = setup(&scratch);
        let source = source_file(&scratch, "spec.pdf", "hello");

        let id = attach(&mut state, b, &source, true);
        let attachment = state.attachments_for_task(b).pop().unwrap();
        assert!(!attachment.is_linked);
        let stored = state.attachment_path(&attachment).unwrap();
        assert!(stored.starts_with(
            std::fs::canonicalize(state.paths.attachments_dir(attachment.project_id)).unwrap()
        ));
        assert_eq!(std::fs::read_to_string(&stored).unwrap(), "hello");

        state.remove_attachment(id);
        assert!(!stored.exists(), "copied file removed from the workspace");
        assert!(source.exists(), "the original is never touched");
        assert!(state.attachments_for_task(b).is_empty());
    }

    #[test]
    fn same_file_name_attached_twice_does_not_collide() {
        let scratch = Scratch::new();
        let (mut state, _a, b) = setup(&scratch);
        let one = source_file(&scratch, "notes.txt", "one");
        attach(&mut state, b, &one, true);
        attach(&mut state, b, &one, true);
        let paths: Vec<_> = state
            .attachments_for_task(b)
            .iter()
            .map(|a| a.file_path.clone())
            .collect();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1]);
    }

    #[test]
    fn linked_attachment_keeps_the_path_and_never_deletes_the_original() {
        let scratch = Scratch::new();
        let (mut state, _a, b) = setup(&scratch);
        let source = source_file(&scratch, "design.png", "png");
        let id = attach(&mut state, b, &source, false);

        let attachment = state.attachments_for_task(b).pop().unwrap();
        assert!(attachment.is_linked);
        assert_eq!(state.attachment_path(&attachment).unwrap(), source);

        state.remove_attachment(id);
        assert!(source.exists());
        assert!(state.attachments.is_empty());
    }

    #[test]
    fn a_tampered_copied_path_cannot_escape_the_attachments_folder() {
        let scratch = Scratch::new();
        let (mut state, _a, b) = setup(&scratch);
        let secret = source_file(&scratch, "secret.txt", "top secret");
        attach(&mut state, b, &secret, true);
        let mut evil = state.attachments_for_task(b).pop().unwrap();
        // As if the db row had been edited by hand.
        // attachments/ -> projects/<id> -> projects -> data -> scratch root,
        // where `secret.txt` really exists, so only the containment check
        // (not a missing file) can reject it.
        evil.file_path = "../../../../secret.txt".into();
        assert!(state
            .paths
            .attachments_dir(evil.project_id)
            .join(&evil.file_path)
            .exists());
        assert!(state.attachment_path(&evil).is_none());
        // And removing it must not delete the file it pointed at.
        state.attachments.push(evil.clone());
        state.remove_attachment(evil.id);
        assert!(secret.exists());
    }

    #[test]
    fn directories_and_missing_files_are_rejected() {
        let scratch = Scratch::new();
        let (mut state, _a, b) = setup(&scratch);
        assert!(state
            .prepare_attachment(Some(b), scratch.path(), true)
            .is_err());
        let missing = scratch.path().join("nope.txt");
        assert!(state.prepare_attachment(Some(b), &missing, true).is_err());
        assert!(state
            .prepare_attachment(
                Some(TaskId(9999)),
                &source_file(&scratch, "x.txt", "x"),
                true
            )
            .is_err());
    }

    #[test]
    fn notes_attach_to_tasks_at_any_depth_and_stay_out_of_the_project_list() {
        let scratch = Scratch::new();
        let (mut state, a, b) = setup(&scratch);
        state.create_note("project note");
        let on_a = state.create_note_for(Some(a), "on A").unwrap();
        let on_b = state.create_note_for(Some(b), "on B").unwrap();

        assert_eq!(state.project_notes().len(), 1);
        assert_eq!(state.notes_for_task(a).len(), 1);
        assert_eq!(state.notes_for_task(a)[0].id, on_a);
        assert_eq!(state.notes_for_task(b)[0].id, on_b);
        // Notes are real files the editor can read back.
        assert_eq!(state.read_note_content(on_b).as_deref(), Some(""));
        // A task that is not in the selected project is refused.
        assert!(state.create_note_for(Some(TaskId(9999)), "ghost").is_none());
    }

    #[test]
    fn priority_due_date_and_branch_persist_and_can_be_cleared() {
        let scratch = Scratch::new();
        let (mut state, _a, b) = setup(&scratch);
        let project = state.selected_project.unwrap();
        let reload = |state: &mut AppState| {
            state.select_project(project);
            state.tasks.iter().find(|t| t.id == b).cloned().unwrap()
        };

        state.cycle_task_priority(b);
        state.set_task_due(b, Some(1_793_318_400));
        state.set_task_branch(b, Some("  feature/login  ".into()));
        let task = reload(&mut state);
        assert_eq!(task.priority, TaskPriority::High, "Medium -> High");
        assert_eq!(task.due_at, Some(1_793_318_400));
        assert_eq!(task.branch.as_deref(), Some("feature/login"), "trimmed");

        state.set_task_due(b, None);
        state.set_task_branch(b, Some("   ".into()));
        let task = reload(&mut state);
        assert_eq!(task.due_at, None);
        assert_eq!(task.branch, None, "blank branch clears it");

        for _ in 0..3 {
            state.cycle_task_priority(b);
        }
        assert_eq!(
            reload(&mut state).priority,
            TaskPriority::Medium,
            "full cycle"
        );
    }

    #[test]
    fn tags_persist_across_a_reload_and_feed_the_project_tag_list() {
        let scratch = Scratch::new();
        let (mut state, a, b) = setup(&scratch);
        assert_eq!(state.add_task_tag(a, " Backend "), Ok(true));
        assert_eq!(state.add_task_tag(a, "BACKEND"), Ok(false));
        assert_eq!(state.add_task_tag(b, "backend"), Ok(true));
        assert_eq!(state.add_task_tag(b, "ui"), Ok(true));
        assert!(state.add_task_tag(b, "a,b").is_err());
        assert!(state.add_task_tag(TaskId(9999), "x").is_err());

        // Counts group ignoring case and the first spelling is shown.
        assert_eq!(
            state.project_tags(),
            vec![("Backend".to_string(), 2), ("ui".to_string(), 1)]
        );

        // A reload from the database sees the same tags.
        let project = state.selected_project.unwrap();
        state.select_project(project);
        let tags_of = |s: &AppState, id| s.tasks.iter().find(|t| t.id == id).unwrap().tags.clone();
        assert_eq!(tags_of(&state, a), vec!["Backend".to_string()]);
        assert_eq!(
            tags_of(&state, b),
            vec!["backend".to_string(), "ui".to_string()]
        );

        state.remove_task_tag(b, "BACKEND");
        assert_eq!(tags_of(&state, b), vec!["ui".to_string()]);
        state.select_project(project);
        assert_eq!(
            tags_of(&state, b),
            vec!["ui".to_string()],
            "removal persisted"
        );
        assert_eq!(state.project_tags().len(), 2);
    }

    #[test]
    fn deleting_a_task_removes_its_and_its_subtasks_files() {
        let scratch = Scratch::new();
        let (mut state, a, b) = setup(&scratch);
        let note = state.create_note_for(Some(b), "deep note").unwrap();
        let project = state.selected_project.unwrap();
        let note_file = state
            .paths
            .notes_dir(project)
            .join(&state.notes.iter().find(|n| n.id == note).unwrap().file_path);
        let source = source_file(&scratch, "spec.pdf", "pdf");
        let id = attach(&mut state, b, &source, true);
        let attachment = state
            .attachments
            .iter()
            .find(|x| x.id == id)
            .cloned()
            .unwrap();
        let stored = state.attachment_path(&attachment).unwrap();
        assert!(note_file.exists() && stored.exists());

        // Delete the *parent*: the subtask's files must go too.
        state.delete_task(a);

        assert!(!note_file.exists(), "subtask note file removed");
        assert!(!stored.exists(), "subtask attachment file removed");
        assert!(state.notes.is_empty());
        assert!(state.attachments.is_empty());
        assert!(state.tasks.is_empty());
        assert!(source.exists(), "originals are never touched");
    }
}
