//! App-wide state: the sqlite connection and current selection. Lives in a
//! GPUI entity so views update via `cx.notify()` rather than holding state
//! themselves (CLAUDE.md GPUI guidelines).

use devspace_core::{
    Note, NoteId, NoteKind, Project, ProjectId, Repo, RepoId, Task, TaskId, TaskStatus,
};
use devspace_storage::{NoteRepo, ProjectRepo, RepoRepo, TaskRepo, WorkspacePaths};
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

/// How many commits the dashboard's "recent commits" section shows. Paged
/// rather than loading full history (CLAUDE.md performance rule).
const RECENT_COMMITS_LIMIT: usize = 5;

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
    pub recent_commits: Vec<devspace_git::CommitInfo>,
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
    }

    /// Re-walks the first linked repo's history for the dashboard's "recent
    /// commits" section. Only the first repo for now — CLAUDE.md allows a
    /// project to have several, but merging/sorting histories across repos
    /// is more than this slice needs; revisit if that becomes a real need.
    fn reload_recent_commits(&mut self) {
        self.recent_commits = self
            .repos
            .first()
            .and_then(|repo| {
                devspace_git::recent_commits(std::path::Path::new(&repo.path), RECENT_COMMITS_LIMIT)
                    .ok()
            })
            .unwrap_or_default();
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
        }
    }

    /// Creates a project-level markdown note: writes an empty file under
    /// `<project>/notes/` and inserts the db row pointing at it.
    pub fn create_note(&mut self, title: &str) {
        let Some(project_id) = self.selected_project else {
            return;
        };
        let id = NoteId(Self::next_id(
            &mut self.next_note_id,
            self.notes.iter().map(|n| n.id.0).max().unwrap_or(0),
        ));
        let file_name = format!("{}.md", id.0);
        let Ok(note) = Note::new(id, project_id, NoteKind::Note, title, &file_name, now()) else {
            return;
        };
        if self.paths.ensure_dirs(Some(project_id)).is_err() {
            return;
        }
        let full_path = self.paths.notes_dir(project_id).join(&file_name);
        if std::fs::write(&full_path, "").is_err() {
            return;
        }
        if NoteRepo::insert(&self.conn, &note).is_ok() {
            self.select_project(project_id);
        }
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
        if TaskRepo::delete(&self.conn, id).is_ok() {
            self.tasks.retain(|t| t.id != id && t.parent_id != Some(id));
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
