//! Changes view (CLAUDE.md "Git client"): the working copy of every repo
//! linked to the project in one screen. Files from all repos sit in one list
//! under repo headers, with a diff viewer that stages by file, hunk or line,
//! a commit box that commits to every repo that has staged files, branch
//! switching for one repo or all of them, and stash.
//!
//! All git work runs on background tasks and is applied back to the view
//! when it finishes (CLAUDE.md: never block the UI thread). The pure logic
//! (row layout, line selection, commit targets) is in `changes_model.rs`.

use std::path::PathBuf;

use devspace_core::{CommitLinkStatus, Repo, RepoId, TaskId};
use devspace_git::{
    self as git, DiffSide, FileDiff, GitError, LineSelection, RepoStatus, StashEntry, SwitchMode,
};
use gpui::{prelude::*, App, Context, Entity, UniformListScrollHandle, WeakEntity, Window};

use crate::changes_model::{self, DiffRow};
use crate::settings::SettingsStore;
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;

mod popovers;
mod render;

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// A repo's column in the view: its status and the little bit of UI state
/// that belongs to it.
struct RepoPane {
    repo: Repo,
    status: Option<RepoStatus>,
    error: Option<String>,
    loading: bool,
    collapsed: bool,
    stashes: Vec<StashEntry>,
    /// Branch names, loaded when a branch popover opens.
    branches: Vec<String>,
    /// Bumped per refresh so a slow answer can't overwrite a newer one.
    generation: u64,
}

impl RepoPane {
    fn new(repo: Repo) -> Self {
        Self {
            repo,
            status: None,
            error: None,
            loading: false,
            collapsed: false,
            stashes: Vec::new(),
            branches: Vec::new(),
            generation: 0,
        }
    }

    fn path(&self) -> PathBuf {
        PathBuf::from(&self.repo.path)
    }
}

/// Which repos a branch action applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Repo(RepoId),
    All,
}

pub(crate) enum Popover {
    Branches(Scope),
    Stashes(RepoId),
    Tasks,
}

/// A destructive action waiting for a second click.
pub(crate) enum Confirm {
    DiscardFiles { repo: RepoId, files: Vec<String> },
    DiscardLines { count: usize },
    DeleteBranch { repo: RepoId, name: String },
    DropStash { repo: RepoId, index: usize },
}

/// What the branch text box does on Enter.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum BranchInput {
    Create,
    Rename { repo: RepoId, from: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Ok,
    Warn,
    Err,
}

#[derive(Debug, Clone)]
pub(crate) struct NoticeLine {
    pub text: String,
    pub tone: Tone,
}

impl NoticeLine {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Ok,
        }
    }
    fn warn(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Warn,
        }
    }
    fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Err,
        }
    }
}

/// The diff of the selected file on the selected side.
pub(crate) struct LoadedDiff {
    pub repo: RepoId,
    pub side: DiffSide,
    pub diff: FileDiff,
    pub rows: Vec<DiffRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineOp {
    Stage,
    Unstage,
    Discard,
}

pub struct ChangesView {
    state: Entity<AppState>,
    settings: Entity<SettingsStore>,
    theme: GlassTheme,
    pub on_close: Option<CloseHandler>,

    repos: Vec<RepoPane>,
    repo_filter: Option<RepoId>,
    list_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    /// Cached row layout of the file list, rebuilt on render.
    rows: Vec<changes_model::Row>,

    selected: Option<(RepoId, String)>,
    side: DiffSide,
    diff: Option<LoadedDiff>,
    diff_error: Option<String>,
    diff_loading: bool,
    diff_generation: u64,
    line_sel: LineSelection,
    line_anchor: Option<(usize, usize)>,

    summary: Entity<TextInput>,
    description: Entity<TextInput>,
    co_authors: Entity<TextInput>,
    amend: bool,
    task_link: Option<TaskId>,
    busy: bool,
    /// Debug QA hook (see `qa.rs`): once status has loaded, select the first
    /// changed file and then show this extra state (`branches`, `stash`, ...).
    pub(crate) qa: Option<String>,

    popover: Option<Popover>,
    branch_filter: Entity<TextInput>,
    branch_input: Entity<TextInput>,
    branch_input_mode: BranchInput,
    stash_message: Entity<TextInput>,
    task_filter: Entity<TextInput>,
    switch_mode: SwitchMode,
    confirm: Option<Confirm>,
    notice: Vec<NoticeLine>,
}

impl ChangesView {
    pub fn new(
        cx: &mut Context<Self>,
        state: Entity<AppState>,
        settings: Entity<SettingsStore>,
        theme: GlassTheme,
    ) -> Self {
        let summary = cx.new(|cx| TextInput::new(cx, "Summary (required)", theme));
        let description = cx.new(|cx| TextInput::new(cx, "Description", theme));
        let co_authors = cx.new(|cx| TextInput::new(cx, "Co-authors: Name <email>, …", theme));
        let branch_filter = cx.new(|cx| TextInput::new(cx, "Filter branches…", theme));
        let branch_input = cx.new(|cx| TextInput::new(cx, "New branch name", theme));
        let stash_message = cx.new(|cx| TextInput::new(cx, "Stash message (optional)", theme));
        let task_filter = cx.new(|cx| TextInput::new(cx, "Search tasks…", theme));
        for input in [
            &summary,
            &description,
            &co_authors,
            &branch_filter,
            &task_filter,
        ] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }

        // Enter in the branch box creates (or renames) the branch; in the
        // stash box it stashes. The handlers go through the entity so they
        // can reach the view.
        let weak: WeakEntity<Self> = cx.weak_entity();
        branch_input.update(cx, |input, _| {
            let weak = weak.clone();
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                let weak = weak.clone();
                cx.defer(move |cx| {
                    weak.update(cx, |this, cx| this.submit_branch_input(text, cx))
                        .ok();
                });
            }));
        });
        stash_message.update(cx, |input, _| {
            let weak = weak.clone();
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                let weak = weak.clone();
                cx.defer(move |cx| {
                    weak.update(cx, |this, cx| this.stash_save(text, cx)).ok();
                });
            }));
        });

        cx.observe(&state, |this, _, cx| {
            this.sync_repos(cx);
            cx.notify();
        })
        .detach();

        let mut view = Self {
            state,
            settings,
            theme,
            on_close: None,
            repos: Vec::new(),
            repo_filter: None,
            list_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            rows: Vec::new(),
            selected: None,
            side: DiffSide::Unstaged,
            diff: None,
            diff_error: None,
            diff_loading: false,
            diff_generation: 0,
            line_sel: LineSelection::new(),
            line_anchor: None,
            summary,
            description,
            co_authors,
            amend: false,
            task_link: None,
            busy: false,
            qa: None,
            popover: None,
            branch_filter,
            branch_input,
            branch_input_mode: BranchInput::Create,
            stash_message,
            task_filter,
            switch_mode: SwitchMode::Bring,
            confirm: None,
            notice: Vec::new(),
        };
        view.sync_repos(cx);
        view
    }

    // ---- plumbing ------------------------------------------------------

    /// Runs `work` on the background executor and hands its result to `done`
    /// on the UI thread (unless the view is gone by then).
    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| done(this, result, cx));
        })
        .detach();
    }

    fn pane(&self, id: RepoId) -> Option<&RepoPane> {
        self.repos.iter().find(|p| p.repo.id == id)
    }

    fn pane_mut(&mut self, id: RepoId) -> Option<&mut RepoPane> {
        self.repos.iter_mut().find(|p| p.repo.id == id)
    }

    fn repo_name(&self, id: RepoId) -> String {
        self.pane(id)
            .map(|p| p.repo.name.clone())
            .unwrap_or_else(|| "repo".into())
    }

    /// Keeps the panes in step with the project's linked repos.
    fn sync_repos(&mut self, cx: &mut Context<Self>) {
        let repos = self.state.read(cx).repos.clone();
        let same = repos.len() == self.repos.len()
            && repos
                .iter()
                .zip(&self.repos)
                .all(|(a, b)| a.id == b.repo.id && a.path == b.repo.path && a.name == b.repo.name);
        if same {
            return;
        }
        let mut old: std::collections::HashMap<RepoId, RepoPane> =
            self.repos.drain(..).map(|p| (p.repo.id, p)).collect();
        self.repos = repos
            .into_iter()
            .map(|repo| match old.remove(&repo.id) {
                Some(mut pane) => {
                    pane.repo = repo;
                    pane
                }
                None => RepoPane::new(repo),
            })
            .collect();
        if self.repo_filter.is_some_and(|f| self.pane(f).is_none()) {
            self.repo_filter = None;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|(r, _)| self.pane(*r).is_none())
        {
            self.clear_selection();
        }
        self.popover = None;
        self.confirm = None;
        self.refresh_all(cx);
    }

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<RepoId> = self.repos.iter().map(|p| p.repo.id).collect();
        for id in ids {
            self.refresh_repo(id, cx);
        }
    }

    /// Re-reads one repo's status and stash list.
    fn refresh_repo(&mut self, id: RepoId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_mut(id) else {
            return;
        };
        pane.loading = true;
        pane.generation += 1;
        let generation = pane.generation;
        let path = pane.path();
        self.run(
            cx,
            move || (git::repo_status(&path), git::stash_list(&path)),
            move |this, (status, stashes), cx| {
                let Some(pane) = this.pane_mut(id) else {
                    return;
                };
                if pane.generation != generation {
                    return;
                }
                pane.loading = false;
                match status {
                    Ok(status) => {
                        pane.status = Some(status);
                        pane.error = None;
                    }
                    Err(e) => {
                        pane.status = None;
                        pane.error = Some(e.to_string());
                    }
                }
                pane.stashes = stashes.unwrap_or_default();
                this.after_status(id, cx);
                cx.notify();
            },
        );
    }

    /// The selected file may have vanished or changed sides after a refresh.
    fn after_status(&mut self, id: RepoId, cx: &mut Context<Self>) {
        if self.qa.is_some() && self.selected.is_none() {
            self.qa_apply(cx);
            return;
        }
        let Some((repo, file)) = self.selected.clone() else {
            return;
        };
        if repo != id {
            return;
        }
        let entry = self
            .pane(id)
            .and_then(|p| p.status.as_ref())
            .and_then(|s| s.files.iter().find(|f| f.path == file))
            .map(|f| (f.unstaged.is_some(), f.staged.is_some()));
        match entry {
            None => self.clear_selection(),
            Some((has_unstaged, has_staged)) => {
                self.side = changes_model::pick_side(self.side, has_unstaged, has_staged);
                self.reload_diff(cx);
            }
        }
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.diff = None;
        self.diff_error = None;
        self.diff_loading = false;
        self.diff_generation += 1;
        self.line_sel.clear();
        self.line_anchor = None;
    }

    fn select_file(&mut self, repo: RepoId, path: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&(repo, path.clone())) {
            return;
        }
        let sides = self
            .pane(repo)
            .and_then(|p| p.status.as_ref())
            .and_then(|s| s.files.iter().find(|f| f.path == path))
            .map(|f| (f.unstaged.is_some(), f.staged.is_some()))
            .unwrap_or((true, false));
        self.side = changes_model::pick_side(DiffSide::Unstaged, sides.0, sides.1);
        self.selected = Some((repo, path));
        self.line_sel.clear();
        self.line_anchor = None;
        self.diff = None;
        self.reload_diff(cx);
        cx.notify();
    }

    fn set_side(&mut self, side: DiffSide, cx: &mut Context<Self>) {
        if self.side == side {
            return;
        }
        self.side = side;
        self.line_sel.clear();
        self.line_anchor = None;
        self.diff = None;
        self.reload_diff(cx);
        cx.notify();
    }

    fn reload_diff(&mut self, cx: &mut Context<Self>) {
        let Some((repo, file)) = self.selected.clone() else {
            self.diff = None;
            return;
        };
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let side = self.side;
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.diff_loading = true;
        self.run(
            cx,
            {
                let file = file.clone();
                move || git::file_diff(&path, &file, side)
            },
            move |this, result, cx| {
                if this.diff_generation != generation {
                    return;
                }
                this.diff_loading = false;
                match result {
                    Ok(diff) => {
                        changes_model::retain_valid(&diff, &mut this.line_sel);
                        if this.qa.as_deref() == Some("lines") {
                            this.qa = None;
                            this.line_sel = changes_model::changed_lines(&diff)
                                .into_iter()
                                .take(3)
                                .collect();
                        }
                        let rows = changes_model::diff_rows(&diff);
                        this.diff = Some(LoadedDiff {
                            repo,
                            side,
                            diff,
                            rows,
                        });
                        this.diff_error = None;
                    }
                    Err(e) => {
                        this.diff = None;
                        this.diff_error = Some(e.to_string());
                    }
                }
                cx.notify();
            },
        );
    }

    fn set_notice(&mut self, lines: Vec<NoticeLine>, cx: &mut Context<Self>) {
        self.notice = lines;
        cx.notify();
    }

    fn error_notice(&mut self, repo: &str, error: impl std::fmt::Display, cx: &mut Context<Self>) {
        self.set_notice(vec![NoticeLine::err(format!("{repo}: {error}"))], cx);
    }

    // ---- staging -------------------------------------------------------

    /// Runs a git change on one repo, reports a failure, and refreshes it.
    fn mutate(
        &mut self,
        repo: RepoId,
        cx: &mut Context<Self>,
        work: impl FnOnce(&std::path::Path) -> Result<(), GitError> + Send + 'static,
    ) {
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.run(
            cx,
            move || work(&path),
            move |this, result, cx| {
                if let Err(e) = result {
                    this.error_notice(&name, e, cx);
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    /// Opens a changed file in the configured IDE (Settings → IDE command).
    fn open_file(&mut self, repo: RepoId, file: String, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(repo) else { return };
        let root = pane.path();
        let name = pane.repo.name.clone();
        let command = self.settings.read(cx).get().ide_command.clone();
        self.run(
            cx,
            move || crate::ide::open_file_in_ide(&command, &root, &file),
            move |this, result, cx| {
                if let Err(message) = result {
                    this.error_notice(&name, message, cx);
                }
            },
        );
    }

    fn toggle_file(&mut self, repo: RepoId, file: String, cx: &mut Context<Self>) {
        let staged = self
            .pane(repo)
            .and_then(|p| p.status.as_ref())
            .and_then(|s| s.files.iter().find(|f| f.path == file))
            .is_some_and(|f| f.is_fully_staged());
        self.mutate(repo, cx, move |path| {
            if staged {
                git::unstage_files(path, &[file])
            } else {
                git::stage_files(path, &[file])
            }
        });
    }

    /// The repo-header checkbox: stage every file, or unstage them all once
    /// everything is staged.
    fn toggle_repo(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(status) = self.pane(repo).and_then(|p| p.status.as_ref()) else {
            return;
        };
        let stage = changes_model::header_toggle_stages(status);
        let files: Vec<String> = status.files.iter().map(|f| f.path.clone()).collect();
        if files.is_empty() {
            return;
        }
        self.mutate(repo, cx, move |path| {
            if stage {
                git::stage_files(path, &files)
            } else {
                git::unstage_files(path, &files)
            }
        });
    }

    fn apply_lines(&mut self, op: LineOp, selection: LineSelection, cx: &mut Context<Self>) {
        let Some(loaded) = &self.diff else { return };
        if selection.is_empty() {
            return;
        }
        let diff = loaded.diff.clone();
        let repo = loaded.repo;
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.run(
            cx,
            move || match op {
                LineOp::Stage => git::stage_lines(&path, &diff, &selection).map(|_| None),
                LineOp::Unstage => git::unstage_lines(&path, &diff, &selection).map(|_| None),
                LineOp::Discard => git::discard_lines(&path, &diff, &selection),
            },
            move |this, result, cx| {
                this.line_sel.clear();
                this.line_anchor = None;
                match result {
                    Ok(Some(backup)) => this.set_notice(
                        vec![NoticeLine::ok(format!(
                            "{name}: discarded. A copy of the file is in {}",
                            backup.display()
                        ))],
                        cx,
                    ),
                    Ok(None) => {}
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    fn stage_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(loaded) = &self.diff else { return };
        let selection = loaded.diff.hunk_lines(hunk);
        let op = if loaded.side == DiffSide::Staged {
            LineOp::Unstage
        } else {
            LineOp::Stage
        };
        self.apply_lines(op, selection, cx);
    }

    fn discard_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(loaded) = &self.diff else { return };
        self.line_sel = loaded.diff.hunk_lines(hunk);
        self.confirm = Some(Confirm::DiscardLines {
            count: self.line_sel.len(),
        });
        cx.notify();
    }

    fn click_line(&mut self, hunk: usize, line: usize, shift: bool, cx: &mut Context<Self>) {
        let Some(loaded) = &self.diff else { return };
        self.line_anchor = changes_model::click_line(
            &loaded.diff,
            &mut self.line_sel,
            self.line_anchor,
            (hunk, line),
            shift,
        );
        cx.notify();
    }

    fn ask_discard_files(&mut self, repo: RepoId, files: Vec<String>, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        self.confirm = Some(Confirm::DiscardFiles { repo, files });
        cx.notify();
    }

    /// Carries out the pending [`Confirm`].
    fn confirm_action(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        match confirm {
            Confirm::DiscardFiles { repo, files } => {
                let Some(pane) = self.pane(repo) else { return };
                let path = pane.path();
                let name = pane.repo.name.clone();
                let count = files.len();
                self.run(
                    cx,
                    move || git::discard_files(&path, &files),
                    move |this, result, cx| {
                        match result {
                            Ok(backup) => this.set_notice(
                                vec![NoticeLine::ok(match backup {
                                    Some(dir) => format!(
                                        "{name}: discarded {count} file{}. Copies are in {}",
                                        if count == 1 { "" } else { "s" },
                                        dir.display()
                                    ),
                                    None => format!("{name}: discarded"),
                                })],
                                cx,
                            ),
                            Err(e) => this.error_notice(&name, e, cx),
                        }
                        this.refresh_repo(repo, cx);
                    },
                );
            }
            Confirm::DiscardLines { .. } => {
                let selection = self.line_sel.clone();
                self.apply_lines(LineOp::Discard, selection, cx);
            }
            Confirm::DeleteBranch { repo, name } => self.delete_branch(repo, name, true, cx),
            Confirm::DropStash { repo, index } => {
                self.mutate(repo, cx, move |path| git::stash_drop(path, index));
            }
        }
        cx.notify();
    }

    // ---- commit --------------------------------------------------------

    /// The repo an amend would apply to: the filtered repo, or the only one.
    pub(crate) fn amend_target(&self) -> Option<RepoId> {
        self.repo_filter
            .or_else(|| (self.repos.len() == 1).then(|| self.repos[0].repo.id))
    }

    /// Repos the next commit goes to.
    pub(crate) fn commit_targets(&self) -> Vec<RepoId> {
        if self.amend {
            return self.amend_target().into_iter().collect();
        }
        let statuses: Vec<(RepoId, &RepoStatus)> = self
            .repos
            .iter()
            .filter_map(|p| p.status.as_ref().map(|s| (p.repo.id, s)))
            .collect();
        changes_model::commit_targets(&statuses, self.repo_filter)
    }

    fn set_amend(&mut self, on: bool, cx: &mut Context<Self>) {
        self.amend = on && self.amend_target().is_some();
        if !self.amend {
            cx.notify();
            return;
        }
        // Pre-fill the message of the commit being amended.
        let Some(pane) = self.amend_target().and_then(|id| self.pane(id)) else {
            return;
        };
        let path = pane.path();
        self.run(
            cx,
            move || git::last_commit_message(&path),
            |this, result, cx| {
                if let Ok(Some((summary, description))) = result {
                    this.summary.update(cx, |i, cx| {
                        i.set_text_selected(summary);
                        cx.notify();
                    });
                    this.description.update(cx, |i, cx| {
                        i.set_text_selected(description);
                        cx.notify();
                    });
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn clear_commit_box(&mut self, cx: &mut Context<Self>) {
        for input in [&self.summary, &self.description, &self.co_authors] {
            input.update(cx, |i, cx| {
                i.set_text_selected("");
                cx.notify();
            });
        }
        self.amend = false;
    }

    pub(crate) fn can_commit(&self, cx: &App) -> bool {
        !self.busy
            && !self.summary.read(cx).content.trim().is_empty()
            && (!self.commit_targets().is_empty())
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let summary = self.summary.read(cx).content.to_string();
        if summary.trim().is_empty() {
            self.set_notice(vec![NoticeLine::err("Write a summary for the commit.")], cx);
            return;
        }
        let description = self.description.read(cx).content.to_string();
        let co_authors: Vec<String> = self
            .co_authors
            .read(cx)
            .content
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let amend = self.amend;
        let targets: Vec<(RepoId, String, PathBuf)> = self
            .commit_targets()
            .into_iter()
            .filter_map(|id| self.pane(id).map(|p| (id, p.repo.name.clone(), p.path())))
            .collect();
        if targets.is_empty() {
            self.set_notice(vec![NoticeLine::err("Nothing is staged.")], cx);
            return;
        }
        self.busy = true;
        cx.notify();
        self.run(
            cx,
            move || {
                targets
                    .into_iter()
                    .map(|(id, name, path)| {
                        let result = git::commit_staged(
                            &path,
                            &git::CommitRequest {
                                summary: &summary,
                                description: &description,
                                co_authors: &co_authors,
                                amend,
                            },
                        );
                        (id, name, result)
                    })
                    .collect::<Vec<_>>()
            },
            move |this, results, cx| {
                this.busy = false;
                let mut lines = Vec::new();
                let mut made: Vec<(RepoId, String)> = Vec::new();
                for (id, name, result) in &results {
                    match result {
                        Ok(info) => {
                            lines.push(NoticeLine::ok(format!(
                                "{name}: committed {} · {}",
                                &info.hash[..7.min(info.hash.len())],
                                info.summary
                            )));
                            made.push((*id, info.hash.clone()));
                        }
                        Err(e) => lines.push(NoticeLine::err(format!("{name}: {e}"))),
                    }
                }
                if !made.is_empty() {
                    this.clear_commit_box(cx);
                    if let Some(task) = this.task_link {
                        let linked = this.state.update(cx, |state, cx| {
                            let result =
                                state.link_commits(task, &made, CommitLinkStatus::Linked, None);
                            cx.notify();
                            result
                        });
                        lines.push(match linked {
                            Ok(n) => NoticeLine::ok(format!(
                                "Linked {n} commit{} to the task.",
                                if n == 1 { "" } else { "s" }
                            )),
                            Err(e) => NoticeLine::err(format!("Couldn't link to the task: {e}")),
                        });
                    }
                }
                this.set_notice(lines, cx);
                for (id, _, _) in results {
                    this.refresh_repo(id, cx);
                }
            },
        );
    }

    // ---- branches ------------------------------------------------------

    /// `(id, name, path)` of the repos a scope covers.
    fn scope_repos(&self, scope: Scope) -> Vec<(RepoId, String, PathBuf)> {
        self.repos
            .iter()
            .filter(|p| match scope {
                Scope::All => true,
                Scope::Repo(id) => p.repo.id == id,
            })
            .map(|p| (p.repo.id, p.repo.name.clone(), p.path()))
            .collect()
    }

    pub(crate) fn open_branches(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.popover = Some(Popover::Branches(scope));
        self.branch_input_mode = BranchInput::Create;
        self.confirm = None;
        for input in [&self.branch_filter, &self.branch_input] {
            input.update(cx, |i, cx| {
                i.set_text_selected("");
                cx.notify();
            });
        }
        for (id, ..) in self.scope_repos(scope) {
            let Some(pane) = self.pane(id) else { continue };
            let repo = pane.repo.clone();
            self.run(
                cx,
                move || git::list_branches(&repo).unwrap_or_default(),
                move |this, branches, cx| {
                    if let Some(p) = this.pane_mut(id) {
                        p.branches = branches;
                    }
                    cx.notify();
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn close_popover(&mut self, cx: &mut Context<Self>) {
        self.popover = None;
        cx.notify();
    }

    /// Switches the scope's repos to `name`. Repos without that branch are
    /// reported and skipped, not treated as failures.
    pub(crate) fn switch_branch(&mut self, scope: Scope, name: String, cx: &mut Context<Self>) {
        let targets = self.scope_repos(scope);
        let mode = self.switch_mode;
        self.popover = None;
        self.run(
            cx,
            move || {
                targets
                    .into_iter()
                    .map(|(id, repo_name, path)| {
                        (
                            id,
                            repo_name,
                            git::switch_branch(&path, &name, mode),
                            name.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            },
            move |this, results, cx| this.finish_branch_op(scope, results, cx),
        );
        cx.notify();
    }

    /// Creates `name` in the scope's repos (from each repo's current commit)
    /// and switches to it, or, for `create_if_missing = false`, only switches
    /// where it exists.
    fn create_branch(&mut self, scope: Scope, name: String, cx: &mut Context<Self>) {
        let targets = self.scope_repos(scope);
        let mode = self.switch_mode;
        self.popover = None;
        self.run(
            cx,
            move || {
                targets
                    .into_iter()
                    .map(|(id, repo_name, path)| {
                        (
                            id,
                            repo_name,
                            git::create_branch(&path, &name, None, mode),
                            name.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            },
            move |this, results, cx| this.finish_branch_op(scope, results, cx),
        );
        cx.notify();
    }

    /// Switch every repo to the task's branch, creating it where it's
    /// missing: the "one branch per task across frontend and backend" move.
    pub(crate) fn use_task_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let targets = self.scope_repos(Scope::All);
        let mode = self.switch_mode;
        self.popover = None;
        self.run(
            cx,
            move || {
                targets
                    .into_iter()
                    .map(|(id, repo_name, path)| {
                        let result = match git::switch_branch(&path, &name, mode) {
                            Err(GitError::Invalid(m)) if m.starts_with("there is no branch") => {
                                git::create_branch(&path, &name, None, mode)
                            }
                            other => other,
                        };
                        (id, repo_name, result, name.clone())
                    })
                    .collect::<Vec<_>>()
            },
            move |this, results, cx| this.finish_branch_op(Scope::All, results, cx),
        );
        cx.notify();
    }

    fn finish_branch_op(
        &mut self,
        scope: Scope,
        results: Vec<(RepoId, String, Result<git::SwitchOutcome, GitError>, String)>,
        cx: &mut Context<Self>,
    ) {
        let mut lines = Vec::new();
        for (id, repo, result, branch) in &results {
            lines.push(match result {
                Ok(out) => {
                    let mut text = format!("{repo}: on {}", out.branch);
                    if out.stashed {
                        text.push_str(" (your changes were stashed)");
                    }
                    if out.created_from_remote {
                        text.push_str(" (tracking origin)");
                    }
                    NoticeLine::ok(text)
                }
                Err(GitError::Invalid(m))
                    if scope == Scope::All && m.starts_with("there is no branch") =>
                {
                    NoticeLine::warn(format!("{repo}: has no branch `{branch}`, left as it was"))
                }
                Err(e) => NoticeLine::err(format!("{repo}: {e}")),
            });
            let _ = id;
        }
        self.set_notice(lines, cx);
        for (id, ..) in results {
            self.refresh_repo(id, cx);
        }
    }

    /// Enter in the branch box: create or rename, per the current mode.
    fn submit_branch_input(&mut self, text: String, cx: &mut Context<Self>) {
        let name = text.trim().to_owned();
        if name.is_empty() {
            return;
        }
        match (&self.popover, self.branch_input_mode.clone()) {
            (Some(Popover::Branches(_)), BranchInput::Rename { repo, from }) => {
                self.branch_input_mode = BranchInput::Create;
                self.mutate(repo, cx, move |path| git::rename_branch(path, &from, &name));
                self.popover = None;
                cx.notify();
            }
            (Some(Popover::Branches(scope)), BranchInput::Create) => {
                let scope = *scope;
                self.create_branch(scope, name, cx);
            }
            _ => {}
        }
    }

    pub(crate) fn start_rename(&mut self, repo: RepoId, from: String, cx: &mut Context<Self>) {
        self.branch_input_mode = BranchInput::Rename {
            repo,
            from: from.clone(),
        };
        self.branch_input.update(cx, |i, cx| {
            i.set_text_selected(from);
            cx.notify();
        });
        cx.notify();
    }

    pub(crate) fn delete_branch(
        &mut self,
        repo: RepoId,
        name: String,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let repo_name = pane.repo.name.clone();
        self.popover = None;
        self.run(
            cx,
            {
                let name = name.clone();
                move || git::delete_branch(&path, &name, force)
            },
            move |this, result, cx| {
                match result {
                    Ok(()) => this.set_notice(
                        vec![NoticeLine::ok(format!(
                            "{repo_name}: deleted branch {name}"
                        ))],
                        cx,
                    ),
                    Err(GitError::NotMerged(_)) => {
                        this.confirm = Some(Confirm::DeleteBranch { repo, name });
                        cx.notify();
                    }
                    Err(e) => this.error_notice(&repo_name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    // ---- stash ---------------------------------------------------------

    pub(crate) fn open_stashes(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        self.popover = Some(Popover::Stashes(repo));
        self.confirm = None;
        self.stash_message.update(cx, |i, cx| {
            i.set_text_selected("");
            cx.notify();
        });
        cx.notify();
    }

    fn stash_save(&mut self, message: String, cx: &mut Context<Self>) {
        let Some(Popover::Stashes(repo)) = self.popover else {
            return;
        };
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.run(
            cx,
            move || git::stash_save(&path, Some(&message), true),
            move |this, result, cx| {
                match result {
                    Ok(true) => this
                        .set_notice(vec![NoticeLine::ok(format!("{name}: changes stashed"))], cx),
                    Ok(false) => this.set_notice(
                        vec![NoticeLine::warn(format!("{name}: nothing to stash"))],
                        cx,
                    ),
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    pub(crate) fn stash_apply(
        &mut self,
        repo: RepoId,
        index: usize,
        pop: bool,
        cx: &mut Context<Self>,
    ) {
        self.mutate(repo, cx, move |path| {
            if pop {
                git::stash_pop(path, index)
            } else {
                git::stash_apply(path, index)
            }
        });
        self.popover = None;
        cx.notify();
    }

    // ---- task link -----------------------------------------------------

    pub(crate) fn set_task_link(&mut self, task: Option<TaskId>, cx: &mut Context<Self>) {
        self.task_link = task;
        self.popover = None;
        cx.notify();
    }
}

#[cfg(debug_assertions)]
impl ChangesView {
    /// Debug-only: puts the view in a given state for screenshot QA, since
    /// there is no synthetic input (see `qa.rs`). Waits until every repo has
    /// loaded so the first file chosen is deterministic.
    fn qa_apply(&mut self, cx: &mut Context<Self>) {
        if self
            .repos
            .iter()
            .any(|p| p.status.is_none() && p.error.is_none())
        {
            return;
        }
        let first = self.repos.iter().find_map(|p| {
            let file = p.status.as_ref()?.files.first()?;
            Some((p.repo.id, file.path.clone()))
        });
        let extra = self.qa.take().unwrap_or_default();
        if let Some((repo, path)) = first.clone() {
            self.select_file(repo, path.clone(), cx);
            if extra == "confirm" {
                self.ask_discard_files(repo, vec![path], cx);
            }
        }
        match extra.as_str() {
            "branches" => self.open_branches(Scope::All, cx),
            "repo-branches" => {
                if let Some((repo, _)) = first {
                    self.open_branches(Scope::Repo(repo), cx);
                }
            }
            "stash" => {
                if let Some((repo, _)) = first {
                    self.open_stashes(repo, cx);
                }
            }
            "tasks" => self.popover = Some(Popover::Tasks),
            "lines" => self.qa = Some("lines".into()),
            _ => {}
        }
        cx.notify();
    }
}

#[cfg(not(debug_assertions))]
impl ChangesView {
    fn qa_apply(&mut self, _cx: &mut Context<Self>) {
        self.qa = None;
    }
}
