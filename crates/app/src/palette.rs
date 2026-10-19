//! Command palette overlay (Ctrl+K / Ctrl+Shift+P) and global search.
//!
//! The view only holds state and draws it. Ranking and grouping live in
//! `palette_rank` (pure, unit-tested); cross-project task/note lookup lives
//! in `devspace-storage`; commit lookup uses `devspace_git::search_history`.
//! All of that I/O runs on a background task, debounced, and results are
//! applied only if the query has not changed since (epoch check).
//!
//! The navigation side (`impl Root` bow) is kept here rather than in
//! `main.rs` so the palette stays one self-contained module.

use crate::theme::TypeScale;
use std::path::PathBuf;
use std::time::Duration;

use devspace_core::{NoteId, ProjectId, Repo};
use devspace_git::RepoCommit;
use devspace_storage::{NoteHit, NoteRepo, NoteTextHit, TaskHit, TaskRepo, WorkspacePaths};
use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    ScrollHandle, SharedString, Window,
};

use crate::motion;
use crate::palette_rank::{
    assemble, step_highlight, Command, Group, Item, Sources, Target, MAX_RESULTS,
};
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;
use crate::ui::{icon, kbd, section_label, Icon};
use crate::Root;

/// Wait this long after the last keystroke before hitting the database and
/// git, so fast typing doesn't spawn a query per character.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// Candidates fetched per source from storage/git; the ranker then keeps
/// the best few per group.
const CANDIDATES: usize = 60;
/// Commits fetched per search.
const COMMIT_LIMIT: usize = 10;

/// Called with the chosen row's target after the palette has been closed.
pub type RunHandler = Box<dyn Fn(Target, &mut Window, &mut Context<CommandPalette>)>;
/// Called when the palette should go away (Esc, click outside, after run).
pub type CloseHandler = Box<dyn Fn(&mut Window, &mut Context<CommandPalette>)>;

/// What the background search returns.
struct SearchResults {
    tasks: Vec<TaskHit>,
    notes: Vec<NoteHit>,
    /// Notes whose text matched (title matches are left out of this list).
    note_text: Vec<NoteTextHit>,
    commits: Vec<RepoCommit>,
}

/// Most note-text rows fetched per search; the group shows fewer.
const NOTE_TEXT_LIMIT: usize = 12;

/// Runs the I/O half of a search. Opens its own SQLite connection because
/// the app's connection belongs to `AppState` on the UI thread.
fn run_search(
    db: PathBuf,
    paths: WorkspacePaths,
    repos: Vec<Repo>,
    query: String,
) -> SearchResults {
    let blank = query.trim().is_empty();
    let limit = if blank { 5 } else { CANDIDATES };
    let (tasks, notes, note_text) = match devspace_storage::open_connection(&db) {
        Ok(conn) => {
            let notes = NoteRepo::search_titles(&conn, &query, limit).unwrap_or_default();
            let skip: Vec<NoteId> = notes.iter().map(|n| n.note_id).collect();
            let note_text =
                NoteRepo::search_contents(&conn, &paths, &query, &skip, NOTE_TEXT_LIMIT)
                    .unwrap_or_default();
            (
                TaskRepo::search_titles(&conn, &query, limit).unwrap_or_default(),
                notes,
                note_text,
            )
        }
        Err(_) => (Vec::new(), Vec::new(), Vec::new()),
    };
    let commits = if blank || repos.is_empty() {
        Vec::new()
    } else {
        devspace_git::search_history(&repos, COMMIT_LIMIT, None, &query).commits
    };
    SearchResults {
        tasks,
        notes,
        note_text,
        commits,
    }
}

/// The palette overlay entity.
pub struct CommandPalette {
    input: Entity<TextInput>,
    state: Entity<AppState>,
    theme: GlassTheme,
    typed: String,
    epoch: u64,
    items: Vec<Item>,
    highlight: usize,
    scroll: ScrollHandle,
    scroll_pending: bool,
    // Last background results, kept so rows don't flicker between
    // keystrokes; they are re-ranked against the current query each time.
    task_items: Vec<Item>,
    note_items: Vec<Item>,
    note_text_items: Vec<Item>,
    commit_items: Vec<Item>,
    /// Runs the chosen target (set by the owner).
    pub on_run: Option<RunHandler>,
    /// Closes the palette (set by the owner).
    pub on_close: Option<CloseHandler>,
}

impl CommandPalette {
    /// Creates the palette and starts the initial (blank-query) search so
    /// recent tasks and notes appear.
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        let input = cx.new(|cx| TextInput::new(cx, "Search or run a command…", theme));
        // Live search: the input notifies on every edit; compare and debounce.
        cx.observe(&input, |this, input, cx| {
            let typed = input.read(cx).content.to_string();
            if typed == this.typed {
                return;
            }
            this.typed = typed;
            // Commits are never re-ranked locally, so stale ones would
            // mislead; tasks/notes get re-ranked against the new text.
            this.commit_items.clear();
            this.note_text_items.clear();
            this.highlight = 0;
            this.scroll_pending = true;
            this.recompute(cx);
            this.schedule_search(cx);
        })
        .detach();

        let mut palette = Self {
            input,
            state,
            theme,
            typed: String::new(),
            epoch: 0,
            items: Vec::new(),
            highlight: 0,
            scroll: ScrollHandle::new(),
            scroll_pending: false,
            task_items: Vec::new(),
            note_items: Vec::new(),
            note_text_items: Vec::new(),
            commit_items: Vec::new(),
            on_run: None,
            on_close: None,
        };
        palette.recompute(cx);
        palette.schedule_search(cx);
        palette
    }

    /// Focus target for the owner to hand focus to when opening.
    pub fn input_focus(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle.clone()
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        let epoch = self.epoch;
        let query = self.typed.clone();
        let (paths, repos) = {
            let state = self.state.read(cx);
            (state.workspace_paths().clone(), state.repos.clone())
        };
        let debounce = if query.trim().is_empty() {
            Duration::ZERO
        } else {
            SEARCH_DEBOUNCE
        };
        cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
                // Superseded while waiting: skip the work entirely.
                let stale = this.update(cx, |this, _| this.epoch != epoch);
                if stale.unwrap_or(true) {
                    return;
                }
            }
            let results = cx
                .background_executor()
                .spawn(async move { run_search(paths.database_file(), paths, repos, query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.apply(results);
                this.recompute(cx);
            });
        })
        .detach();
    }

    fn apply(&mut self, results: SearchResults) {
        self.task_items = results
            .tasks
            .into_iter()
            .map(|h| Item {
                group: Group::Tasks,
                title: h.title,
                detail: h.project_name,
                target: Target::Task {
                    project: h.project_id,
                    task: h.task_id,
                },
            })
            .collect();
        self.note_items = results
            .notes
            .into_iter()
            .map(|h| Item {
                group: Group::Notes,
                detail: h.project_name,
                target: Target::Note {
                    project: h.project_id,
                    note: h.note_id,
                    task: h.task_id,
                    title: h.title.clone(),
                },
                title: h.title,
            })
            .collect();
        self.note_text_items = results
            .note_text
            .into_iter()
            .map(|h| Item {
                group: Group::NoteText,
                detail: format!("{} · {}", h.hit.project_name, h.snippet),
                target: Target::Note {
                    project: h.hit.project_id,
                    note: h.hit.note_id,
                    task: h.hit.task_id,
                    title: h.hit.title.clone(),
                },
                title: h.hit.title,
            })
            .collect();
        self.commit_items = results
            .commits
            .into_iter()
            .map(|c| Item {
                group: Group::Commits,
                detail: format!(
                    "{} · {} · {}",
                    c.repo_name,
                    c.commit.hash.chars().take(7).collect::<String>(),
                    c.commit.author
                ),
                title: c.commit.summary,
                target: Target::Commit,
            })
            .collect();
    }

    /// Re-ranks everything in memory (cheap) and keeps the highlight valid.
    fn recompute(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<Item> = self
            .state
            .read(cx)
            .projects
            .iter()
            .map(|p| Item {
                group: Group::Projects,
                title: p.name.clone(),
                detail: String::new(),
                target: Target::Project(p.id),
            })
            .collect();
        let sources = Sources {
            commands: Sources::command_items(),
            projects,
            tasks: self.task_items.clone(),
            notes: self.note_items.clone(),
            note_text: self.note_text_items.clone(),
            commits: self.commit_items.clone(),
        };
        self.items = assemble(&self.typed, sources);
        self.highlight = self.highlight.min(self.items.len().saturating_sub(1));
        cx.notify();
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.highlight = step_highlight(self.highlight, delta, self.items.len());
        self.scroll_pending = true;
        cx.notify();
    }

    fn run_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        let target = item.target.clone();
        if let Some(handler) = self.on_run.take() {
            handler(target, window, cx);
            self.on_run = Some(handler);
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self.on_close.take() {
            handler(window, cx);
            self.on_close = Some(handler);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let ctrl = ks.modifiers.control;
        match ks.key.as_str() {
            "down" => self.move_highlight(1, cx),
            "up" => self.move_highlight(-1, cx),
            "n" if ctrl => self.move_highlight(1, cx),
            "p" if ctrl => self.move_highlight(-1, cx),
            "tab" => self.move_highlight(if ks.modifiers.shift { -1 } else { 1 }, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input_focus(cx)
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;

        // Wire Enter/Esc on the input to this palette.
        let palette = cx.entity();
        self.input.update(cx, |input, _| {
            if input.on_confirm.is_none() {
                let palette = palette.clone();
                input.on_confirm = Some(Box::new(move |_text, window, cx| {
                    palette.update(cx, |this, cx| {
                        let index = this.highlight;
                        this.run_at(index, window, cx);
                    });
                }));
            }
            if input.on_cancel.is_none() {
                let palette = palette.clone();
                input.on_cancel = Some(Box::new(move |window, cx| {
                    palette.update(cx, |this, cx| this.close(window, cx));
                }));
            }
        });

        // Rows with group labels interleaved; remember which child the
        // highlighted row is so it can be scrolled into view.
        let mut list = div()
            .id("palette-results")
            .flex()
            .flex_col()
            .max_h(px(380.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(theme.space_1);
        let mut child_ix = 0usize;
        let mut highlight_child = 0usize;
        let mut last_group: Option<Group> = None;
        for (ix, item) in self.items.iter().enumerate().take(MAX_RESULTS) {
            if last_group != Some(item.group) {
                last_group = Some(item.group);
                list = list.child(
                    div()
                        .px(theme.space_3)
                        .pt(theme.space_3)
                        .pb(theme.space_1)
                        .child(section_label(&theme, item.group.label())),
                );
                child_ix += 1;
            }
            let selected = ix == self.highlight;
            if selected {
                highlight_child = child_ix;
            }
            child_ix += 1;
            let row = div()
                .id(("palette-row", ix))
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.space_3)
                .h(px(36.0))
                .px(theme.space_3)
                .rounded(theme.radius_md)
                .cursor_pointer()
                .t_body()
                .text_color(theme.text_primary)
                .when(!selected, |el| el.hover(move |s| s.bg(theme.surface_hover)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_at(ix, window, cx);
                }))
                .child(icon(
                    group_icon(item.group),
                    15.0,
                    if selected {
                        theme.accent
                    } else {
                        theme.text_muted
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(SharedString::from(item.title.clone())),
                )
                .when(!item.detail.is_empty(), |el| {
                    el.child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(220.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(item.detail.clone())),
                    )
                });
            // The highlight slides in as it moves between rows.
            list = list.child(motion::fade(
                row,
                ("palette-row-fade", ix),
                selected,
                theme.accent.opacity(0.14),
                gpui::transparent_black(),
            ));
        }
        if self.items.is_empty() {
            list = list.child(
                div()
                    .px(theme.space_3)
                    .py(theme.space_4)
                    .t_body()
                    .text_color(theme.text_secondary)
                    .child("No results"),
            );
        }
        if self.scroll_pending {
            self.scroll_pending = false;
            self.scroll.scroll_to_item(highlight_child);
        }

        // Backdrop: dims the app and closes on an outside click. The panel
        // stops mouse-down propagation so clicks inside don't reach it.
        div()
            .id("palette-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(80.0))
            .bg(theme.scrim)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .id("palette-panel")
                    .flex()
                    .flex_col()
                    .w(px(600.0))
                    .max_w_full()
                    .gap(theme.space_2)
                    .p(theme.space_2)
                    .rounded(theme.radius_lg)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_strong)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .pl(theme.space_2)
                            .child(icon(Icon::Search, 16.0, theme.text_muted))
                            .child(div().flex_1().child(self.input.clone())),
                    )
                    .child(
                        // Reading area: near-solid, per the readability
                        // rules, not glass.
                        div()
                            .rounded(theme.radius_md)
                            .bg(theme.surface_solid)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .overflow_hidden()
                            .child(list),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_4)
                            .px(theme.space_2)
                            .pb(px(2.0))
                            .t_micro()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_1)
                                    .child(kbd(&theme, "↑↓"))
                                    .child("navigate"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_1)
                                    .child(kbd(&theme, "Enter"))
                                    .child("open"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_1)
                                    .child(kbd(&theme, "Esc"))
                                    .child("close"),
                            ),
                    ),
            )
    }
}

/// The icon that marks a palette group.
fn group_icon(group: Group) -> Icon {
    match group {
        Group::Commands => Icon::Command,
        Group::Projects => Icon::Folder,
        Group::Tasks => Icon::ListChecks,
        Group::Notes | Group::NoteText => Icon::FileText,
        Group::Commits => Icon::GitCommit,
    }
}

// ---- Root integration -------------------------------------------------

impl Root {
    /// Opens the palette, or closes it if already open. Remembers the
    /// focused element so closing can hand focus back.
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        self.palette_prev_focus = window.focused(cx);
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let palette = cx.new(|cx| {
            let mut palette = CommandPalette::new(cx, state, theme);
            let run_root = root.clone();
            palette.on_run = Some(Box::new(move |target, window, cx| {
                run_root.update(cx, |root, cx| {
                    root.close_palette(window, cx);
                    root.run_palette_target(target, window, cx);
                });
            }));
            let close_root = root.clone();
            palette.on_close = Some(Box::new(move |window, cx| {
                close_root.update(cx, |root, cx| root.close_palette(window, cx));
            }));
            palette
        });
        let focus = palette.read(cx).input_focus(cx);
        window.focus(&focus, cx);
        self.palette = Some(palette);
        cx.notify();
    }

    /// Closes the palette and restores focus to what had it before.
    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_none() {
            return;
        }
        let handle = self
            .palette_prev_focus
            .take()
            .unwrap_or_else(|| self.focus_handle.clone());
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Makes `project` the selected one, leaving views that belong to the
    /// previously selected project. No-op for the selection if already
    /// selected, but still clears the history/note/task pages so the
    /// destination shows.
    fn palette_switch_project(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.open_history = None;
        self.open_task = None;
        self.open_note = None;
        self.settings_open = false;
        if self.state.read(cx).selected_project != Some(project) {
            self.state
                .update(cx, |state, _| state.select_project(project));
        }
    }

    /// Runs a built-in command. Views are opened (never toggled closed) so
    /// picking one from the palette always lands on it.
    pub(crate) fn run_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            Command::ToggleTerminal => self.toggle_terminal(window, cx),
            Command::NewTerminal => {
                self.terminal_open = true;
                self.terminal_panel
                    .update(cx, |panel, cx| panel.new_tab(window, cx));
            }
            Command::OpenHistory => {
                if self.open_history.is_none() {
                    self.toggle_history(cx);
                }
            }
            Command::OpenRunCommands => {
                if !self.run_open {
                    self.toggle_run_view(cx);
                }
            }
            Command::OpenTimesheet => {
                if !self.timesheet_open {
                    self.toggle_timesheet_view(cx);
                }
            }
            Command::OpenCredentials => {
                if !self.credentials_open {
                    self.toggle_credentials_view(cx);
                }
            }
            Command::OpenSettings => {
                if !self.settings_open {
                    self.toggle_settings_view(cx);
                }
            }
            Command::NewProject => {
                let focus = self.new_project_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }
            Command::CycleTheme => self.settings.update(cx, |store, cx| {
                store.update(cx, |s| s.theme_mode = s.theme_mode.cycle())
            }),
            Command::ToggleReduceTransparency => self.settings.update(cx, |store, cx| {
                store.update(cx, |s| s.reduce_transparency = !s.reduce_transparency)
            }),
            Command::ToggleReduceMotion => self.settings.update(cx, |store, cx| {
                store.update(cx, |s| s.reduce_motion = !s.reduce_motion)
            }),
        }
    }

    fn run_palette_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Target::Command(command) => self.run_command(command, window, cx),
            Target::Project(project) => self.palette_switch_project(project, cx),
            Target::Task { project, task } => {
                self.palette_switch_project(project, cx);
                self.open_task_detail(task, cx);
            }
            Target::Note {
                project,
                note,
                task,
                title,
            } => {
                self.palette_switch_project(project, cx);
                if let Some(task) = task {
                    // So closing the note returns to its task.
                    self.open_task_detail(task, cx);
                }
                self.open_note(note, title.into(), cx);
            }
            Target::Commit => {
                if self.open_history.is_none() {
                    self.toggle_history(cx);
                }
            }
        }
        cx.notify();
    }
}
