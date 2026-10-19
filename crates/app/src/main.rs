//! GPUI entry point and root view.
//! Pin: zed-industries/zed @ 56934ec0560bce8929025497411de7f5322fb6ca
//! (see CLAUDE.md "GPUI guidelines" — pin an exact revision, don't guess
//! the pre-1.0 API).

mod attach_flow;
mod commit_browser;
mod credentials_panel;
mod date;
mod external;
mod ide;
mod markdown;
mod markdown_view;
mod note_editor;
mod note_format;
mod note_page;
mod palette;
mod palette_rank;
mod project_files;
mod run_panel;
mod settings;
mod state;
mod task_detail;
mod terminal_panel;
mod terminal_view;
mod text_input;
mod theme;
mod timesheet_panel;
mod workspace_tools;

use anyhow::Result;
use commit_browser::CommitBrowser;
use credentials_panel::CredentialsPanel;
use devspace_core::{Note, NoteId, NoteKind, Project, ProjectId, Repo, Task, TaskId, TaskStatus};
use devspace_storage::{run_migrations, WorkspacePaths};
use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Entity, KeyBinding,
    PathPromptOptions, SharedString, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowOptions,
};
use note_editor::NoteEditor;
use project_files::ProjectFiles;
use run_panel::RunPanel;
use settings::SettingsStore;
use state::AppState;
use task_detail::TaskDetail;
use terminal_panel::TerminalPanel;
use text_input::TextInput;
use theme::GlassTheme;
use timesheet_panel::TimesheetPanel;
use workspace_tools::WorkspaceTools;

actions!(devspace, [ToggleTerminal, NewTerminal, OpenPalette]);

struct Root {
    state: Entity<AppState>,
    theme: GlassTheme,
    new_project_input: Entity<TextInput>,
    new_task_input: Entity<TextInput>,
    new_note_input: Entity<TextInput>,
    new_repo_input: Entity<TextInput>,
    editing_project: Option<(ProjectId, Entity<TextInput>)>,
    editing_task: Option<(TaskId, Entity<TextInput>)>,
    editing_note: Option<(NoteId, Entity<TextInput>)>,
    /// The note currently open in the full-content editor, if any — replaces
    /// the task/notes list in `main_panel` while set.
    open_note: Option<(NoteId, SharedString, Entity<NoteEditor>)>,
    /// Edit / Split / Preview for the open note, and its parsed preview
    /// (re-parsed only when the text changes). See `note_page.rs`.
    note_mode: note_page::NoteMode,
    note_preview: Option<(SharedString, Vec<markdown::Block>)>,
    /// Right-hand terminal panel (CLAUDE.md layout concept: "collapsible
    /// panels for terminal, commit browser and credentials"). Owns a tab set
    /// per project; hiding it or switching projects keeps shells running.
    terminal_panel: Entity<TerminalPanel>,
    focus_handle: gpui::FocusHandle,
    /// The commit history browser, replacing the main content while open
    /// (like `open_note`).
    open_history: Option<Entity<CommitBrowser>>,
    /// The task/subtask detail page (notes, attachments, commits). Stays
    /// open underneath the note editor so closing a note returns to it.
    open_task: Option<Entity<TaskDetail>>,
    /// Repo whose unlink is awaiting a second click because it still has
    /// commit mappings (they are deleted along with it).
    confirm_unlink_repo: Option<devspace_core::RepoId>,
    /// Project-page filter: show only tasks carrying this tag.
    tag_filter: Option<String>,
    terminal_open: bool,
    // Command palette overlay (see palette.rs) and the focus to restore on close.
    palette: Option<Entity<palette::CommandPalette>>,
    palette_prev_focus: Option<gpui::FocusHandle>,
    /// Run-commands view (see `run_panel.rs`). The entity lives for the whole
    /// app so commands keep running while the view is closed.
    run_panel: Entity<RunPanel>,
    run_open: bool,
    /// Timesheet view (see `timesheet_panel.rs`), shown in place of the main
    /// content like the run-commands view.
    timesheet_panel: Entity<TimesheetPanel>,
    timesheet_open: bool,
    /// Credentials vault view (see `credentials_panel.rs`), shown in place of
    /// the main content.
    credentials_panel: Entity<CredentialsPanel>,
    credentials_open: bool,
    // --- settings / IDE / backup hooks (settings.rs, workspace_tools.rs) ---
    /// Persisted per-user settings (last project/task, terminal panel, IDE).
    settings: Entity<SettingsStore>,
    /// "Open in IDE" + backup/export/import card on the project page.
    workspace_tools: Entity<WorkspaceTools>,
    /// Project-level attachments section on the project page.
    project_files: Entity<ProjectFiles>,
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "Todo",
        TaskStatus::InProgress => "In Progress",
        TaskStatus::Blocked => "Blocked",
        TaskStatus::Done => "Done",
    }
}

/// Orders tasks depth-first (parents before children, preserving the
/// `created_at` order already applied by the storage query) and tags each
/// with its nesting depth so the UI can indent subtasks under parents.
fn flatten_tasks(tasks: &[Task]) -> Vec<(Task, usize)> {
    fn walk(tasks: &[Task], parent: Option<TaskId>, depth: usize, out: &mut Vec<(Task, usize)>) {
        for task in tasks.iter().filter(|t| t.parent_id == parent) {
            out.push((task.clone(), depth));
            walk(tasks, Some(task.id), depth + 1, out);
        }
    }
    let mut out = Vec::with_capacity(tasks.len());
    walk(tasks, None, 0, &mut out);
    out
}

/// Custom-drawn titlebar that blends into the window (CLAUDE.md layout
/// concept): draggable, no native chrome, matches the glass surfaces below.
fn titlebar(theme: &GlassTheme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(40.0))
        .w_full()
        .px(theme.space_4)
        .gap(theme.space_2)
        .bg(theme.surface)
        .border_b_1()
        .border_color(theme.border_light)
        .window_control_area(WindowControlArea::Drag)
        .child(
            div()
                .size(px(10.0))
                .rounded_full()
                .bg(theme.accent)
                .shadow_sm(),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_secondary)
                .child("Devspace"),
        )
}

/// A small text-glyph icon button (pencil, trash, ...). GPUI has no icon font
/// wired up yet, so these are plain characters styled like buttons.
fn icon_button(
    theme: &GlassTheme,
    glyph: &'static str,
    id: impl Into<gpui::ElementId>,
) -> gpui::Stateful<gpui::Div> {
    icon_button_tinted(theme, glyph, id, theme.text_primary)
}

/// Same as [`icon_button`], but hovering tints toward `hover_color` instead
/// of the default text color — used for destructive actions (delete,
/// archive) so they read as a warning, not just a generic highlight.
fn icon_button_tinted(
    theme: &GlassTheme,
    glyph: &'static str,
    id: impl Into<gpui::ElementId>,
    hover_color: gpui::Hsla,
) -> gpui::Stateful<gpui::Div> {
    let theme = *theme;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.0))
        .rounded(theme.radius_sm)
        .text_color(theme.text_secondary)
        .text_sm()
        .cursor_pointer()
        .shadow_sm()
        .hover(move |s| s.bg(theme.surface_raised).text_color(hover_color))
        .child(glyph)
}

/// A labeled "create new ..." row: a text input plus a hint to press Enter.
fn create_row(theme: &GlassTheme, input: &Entity<TextInput>) -> impl IntoElement {
    let theme = *theme;
    div()
        .mt(theme.space_2)
        .flex()
        .flex_col()
        .gap(theme.space_1)
        .child(input.clone())
        .child(
            div()
                .px(theme.space_1)
                .text_color(theme.text_secondary)
                .text_xs()
                .child("Press Enter to create, Esc to cancel"),
        )
}

/// One chip in the project page's tag filter row.
fn tag_filter_chip(
    theme: &GlassTheme,
    label: String,
    active: bool,
    id: gpui::ElementId,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = *theme;
    div()
        .id(id)
        .px(theme.space_3)
        .py(theme.space_1)
        .rounded(theme.radius_sm)
        .border_1()
        .text_xs()
        .cursor_pointer()
        .when(active, |d| {
            d.border_color(theme.accent)
                .text_color(theme.accent)
                .bg(theme.accent.opacity(0.12))
        })
        .when(!active, |d| {
            d.border_color(theme.border_subtle)
                .text_color(theme.text_secondary)
                .hover(move |s| s.bg(theme.surface_raised))
        })
        .on_click(on_click)
        .child(label)
}

fn stat_tile(
    theme: &GlassTheme,
    label: &'static str,
    count: usize,
    color: gpui::Hsla,
) -> impl IntoElement {
    let theme = *theme;
    div()
        .flex()
        .flex_col()
        .gap(theme.space_1)
        .px(theme.space_3)
        .py(theme.space_2)
        .rounded(theme.radius_md)
        .bg(theme.surface_raised)
        .border_1()
        .border_color(theme.border_subtle)
        .shadow_sm()
        .min_w(px(96.0))
        .child(div().text_2xl().text_color(color).child(count.to_string()))
        .child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(label),
        )
}

/// Project dashboard (CLAUDE.md: "dashboard with open tasks, recent notes,
/// recent commits"; the commit list lives in the history browser instead).
/// Recent notes render separately in `Root::render` (it
/// needs `cx` for the open-note click handler).
fn dashboard(
    theme: &GlassTheme,
    project: &Project,
    tasks: &[Task],
    unreadable_repos: &[String],
) -> impl IntoElement {
    let theme = *theme;
    let todo = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Todo)
        .count();
    let in_progress = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .count();
    let blocked = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Blocked)
        .count();
    let done = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();

    div()
        .flex()
        .flex_col()
        .gap(theme.space_3)
        .pb(theme.space_3)
        .border_b_1()
        .border_color(theme.border_subtle)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(theme.space_2)
                .child(
                    div()
                        .text_xl()
                        .text_color(theme.text_primary)
                        .child(project.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(format!("created {}", date::format_date(project.created_at))),
                ),
        )
        .child(
            div()
                .flex()
                .gap(theme.space_2)
                .child(stat_tile(&theme, "Todo", todo, theme.text_secondary))
                .child(stat_tile(&theme, "In Progress", in_progress, theme.accent))
                .child(stat_tile(&theme, "Blocked", blocked, theme.warning))
                .child(stat_tile(&theme, "Done", done, theme.success)),
        )
        .when(!unreadable_repos.is_empty(), |el| {
            el.child(div().text_xs().text_color(theme.warning).child(format!(
                "Couldn't read history for: {}",
                unreadable_repos.join(", ")
            )))
        })
}

impl Root {
    fn start_editing_project(&mut self, id: ProjectId, name: SharedString, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Project name…", theme);
            input.content = name;
            input
        });
        input.update(cx, |input, _| {
            let state_confirm = state.clone();
            let root_confirm = root.clone();
            input.on_confirm = Some(Box::new(move |name, _window, cx| {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    state_confirm.update(cx, |state, cx| {
                        state.rename_project(id, &name);
                        cx.notify();
                    });
                }
                root_confirm.update(cx, |root, cx| {
                    root.editing_project = None;
                    cx.notify();
                });
            }));
            let root_cancel = root.clone();
            input.on_cancel = Some(Box::new(move |_window, cx| {
                root_cancel.update(cx, |root, cx| {
                    root.editing_project = None;
                    cx.notify();
                });
            }));
        });
        self.editing_project = Some((id, input));
        cx.notify();
    }

    fn start_editing_task(&mut self, id: TaskId, title: SharedString, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Task title…", theme);
            input.content = title;
            input
        });
        input.update(cx, |input, _| {
            let state_confirm = state.clone();
            let root_confirm = root.clone();
            input.on_confirm = Some(Box::new(move |title, _window, cx| {
                let title = title.trim().to_string();
                if !title.is_empty() {
                    state_confirm.update(cx, |state, cx| {
                        state.rename_task(id, &title);
                        cx.notify();
                    });
                }
                root_confirm.update(cx, |root, cx| {
                    root.editing_task = None;
                    cx.notify();
                });
            }));
            let root_cancel = root.clone();
            input.on_cancel = Some(Box::new(move |_window, cx| {
                root_cancel.update(cx, |root, cx| {
                    root.editing_task = None;
                    cx.notify();
                });
            }));
        });
        self.editing_task = Some((id, input));
        cx.notify();
    }

    fn start_editing_note(&mut self, id: NoteId, title: SharedString, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Note title…", theme);
            input.content = title;
            input
        });
        input.update(cx, |input, _| {
            let state_confirm = state.clone();
            let root_confirm = root.clone();
            input.on_confirm = Some(Box::new(move |title, _window, cx| {
                let title = title.trim().to_string();
                if !title.is_empty() {
                    state_confirm.update(cx, |state, cx| {
                        state.rename_note(id, &title);
                        cx.notify();
                    });
                }
                root_confirm.update(cx, |root, cx| {
                    root.editing_note = None;
                    cx.notify();
                });
            }));
            let root_cancel = root.clone();
            input.on_cancel = Some(Box::new(move |_window, cx| {
                root_cancel.update(cx, |root, cx| {
                    root.editing_note = None;
                    cx.notify();
                });
            }));
        });
        self.editing_note = Some((id, input));
        cx.notify();
    }

    fn open_note(&mut self, id: NoteId, title: SharedString, cx: &mut Context<Self>) {
        let content = self
            .state
            .read(cx)
            .read_note_content(id)
            .unwrap_or_default();
        let mode = note_page::NoteMode::initial(&content);
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let editor = cx.new(|cx| {
            let mut editor = NoteEditor::new(cx, content, theme);
            let change_state = state.clone();
            editor.on_change = Some(Box::new(move |content, _window, cx| {
                change_state.update(cx, |state, _| {
                    state.write_note_content(id, content);
                });
            }));
            let close_root = root.clone();
            editor.on_close = Some(Box::new(move |_window, cx| {
                close_root.update(cx, |root, cx| {
                    root.open_note = None;
                    cx.notify();
                });
            }));
            let save_root = root.clone();
            editor.on_save = Some(Box::new(move |window, cx| {
                save_root.update(cx, |root, cx| {
                    root.set_note_mode(note_page::NoteMode::Preview, window, cx);
                });
            }));
            editor
        });
        self.open_note = Some((id, title, editor));
        self.note_mode = mode;
        self.note_preview = None;
        cx.notify();
    }

    /// Opens the detail page (notes, attachments, commits) for a task or
    /// subtask at any depth, in place of the project page.
    fn open_task_detail(&mut self, task: TaskId, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(cx, state, theme, task);
            let close_root = root.clone();
            detail.on_close = Some(Box::new(move |_window, cx| {
                close_root.update(cx, |root, cx| {
                    root.open_task = None;
                    cx.notify();
                });
            }));
            let note_root = root.clone();
            detail.on_open_note = Some(Box::new(move |id, title, _window, cx| {
                note_root.update(cx, |root, cx| root.open_note(id, title, cx));
            }));
            let history_root = root.clone();
            detail.on_open_history = Some(Box::new(move |_window, cx| {
                history_root.update(cx, |root, cx| {
                    root.open_task = None;
                    if root.open_history.is_none() {
                        root.toggle_history(cx);
                    }
                });
            }));
            detail
        });
        self.open_task = Some(detail);
        cx.notify();
    }

    /// Opens the OS folder picker and links the chosen folder as a repo of
    /// the selected project. A subfolder resolves to its repo root so git
    /// history works; a folder outside any repo is still linked and shows
    /// up in the dashboard's "couldn't read history" warning.
    fn browse_repo(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Link repository".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |_this, cx| {
            // Err = picker unavailable, Ok(None) = cancelled.
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(chosen) = paths.into_iter().next() else {
                return;
            };
            let root = devspace_git::repo_root(&chosen).unwrap_or(chosen);
            let path = root.to_string_lossy().to_string();
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            state.update(cx, |state, cx| {
                state.link_repo(&path, &name);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the commit history browser in place of the main content, or
    /// closes it if already open.
    fn toggle_history(&mut self, cx: &mut Context<Self>) {
        if self.open_history.take().is_some() {
            cx.notify();
            return;
        }
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let browser = cx.new(|cx| {
            let mut browser = CommitBrowser::new(cx, state, theme);
            browser.on_close = Some(Box::new(move |_window, cx| {
                root.update(cx, |root, cx| {
                    root.open_history = None;
                    cx.notify();
                });
            }));
            browser
        });
        self.open_note = None;
        self.open_task = None;
        self.run_open = false;
        self.timesheet_open = false;
        self.credentials_open = false;
        self.open_history = Some(browser);
        cx.notify();
    }

    /// Records what should be restored next launch (selected project, open
    /// task page, terminal visibility). The store only writes when a value
    /// actually changed, and debounces, so calling this per render is cheap.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let project = self.state.read(cx).selected_project.map(|p| p.0);
        let task = self.open_task.as_ref().map(|t| t.read(cx).task_id().0);
        let terminal_open = self.terminal_open;
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| {
                s.last_project = project;
                s.last_task = task;
                s.terminal_open = terminal_open;
            })
        });
    }

    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_open = !self.terminal_open;
        if self.terminal_open {
            self.terminal_panel
                .update(cx, |panel, cx| panel.ensure_tab(window, cx));
        } else {
            // The focused terminal is about to leave the tree; hand focus
            // back so shortcuts (including this toggle) keep working.
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_settings(cx);
        let theme = self.theme;
        let state = self.state.read(cx);
        let projects = state.projects.clone();
        let selected_project = state.selected_project;
        let selected_project_data = selected_project
            .and_then(|id| state.projects.iter().find(|p| p.id == id))
            .cloned();
        let raw_tasks = state.tasks.clone();
        let tasks = flatten_tasks(&state.tasks);
        // Tag filter: keep only tasks with the tag (any depth), flattened to
        // one level since their parents may be filtered out. A filter whose
        // tag no longer exists anywhere is ignored rather than showing nothing.
        let project_tags = state.project_tags();
        let active_tag: Option<String> = self.tag_filter.clone().filter(|wanted| {
            project_tags
                .iter()
                .any(|(name, _)| name.to_lowercase() == wanted.to_lowercase())
        });
        let tasks: Vec<(Task, usize)> = match &active_tag {
            Some(tag) => tasks
                .into_iter()
                .filter(|(t, _)| t.has_tag(tag))
                .map(|(t, _)| (t, 0))
                .collect(),
            None => tasks,
        };
        // Per task: (notes, attachments, mapped commits), for row badges.
        let mut task_counts: std::collections::HashMap<TaskId, (usize, usize, usize)> =
            std::collections::HashMap::new();
        for note in state.notes.iter().filter(|n| n.task_id.is_some()) {
            if let Some(t) = note.task_id {
                task_counts.entry(t).or_default().0 += 1;
            }
        }
        for attachment in &state.attachments {
            if let Some(t) = attachment.task_id {
                task_counts.entry(t).or_default().1 += 1;
            }
        }
        for link in &state.commit_links {
            task_counts.entry(link.task_id).or_default().2 += 1;
        }
        let confirm_unlink = self.confirm_unlink_repo;
        // Mapped commits per repo, so unlinking a repo can warn first.
        let mut repo_link_counts: std::collections::HashMap<devspace_core::RepoId, usize> =
            std::collections::HashMap::new();
        for link in &state.commit_links {
            *repo_link_counts.entry(link.repo_id).or_default() += 1;
        }
        let notes = state.project_notes();
        let new_note_kind = state.new_note_kind;
        let repos = state.repos.clone();
        let unreadable_repos = state.unreadable_repos.clone();
        let editing_project_id = self.editing_project.as_ref().map(|(id, _)| *id);
        let editing_task_id = self.editing_task.as_ref().map(|(id, _)| *id);
        let editing_note_id = self.editing_note.as_ref().map(|(id, _)| *id);

        let sidebar = div()
            .flex()
            .flex_col()
            .w(px(280.0))
            .h_full()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border_subtle)
            .p(theme.space_3)
            .gap(theme.space_1)
            .child(
                div()
                    .px(theme.space_2)
                    .pb(theme.space_1)
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child("PROJECTS"),
            )
            .children(projects.into_iter().map(|project| {
                let id = project.id;
                let selected = selected_project == Some(id);

                if editing_project_id == Some(id) {
                    let (_, input) = self.editing_project.as_ref().unwrap();
                    return div()
                        .px(theme.space_1)
                        .py(theme.space_1)
                        .child(input.clone())
                        .into_any_element();
                }

                let name = project.name.clone();
                let terminal_count = self.terminal_panel.read(cx).tab_count(id.0);
                let row = div()
                    .id(("project", id.0 as u64))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space_2)
                    .px(theme.space_3)
                    .py(theme.space_2)
                    .rounded(theme.radius_md)
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(div().size(px(6.0)).rounded_full().bg(if selected {
                                theme.accent
                            } else {
                                gpui::transparent_black()
                            }))
                            .child(
                                div()
                                    .text_color(if selected {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(project.name.clone()),
                            )
                            .when(terminal_count > 0, |el| {
                                el.child(
                                    div()
                                        .px(theme.space_1)
                                        .rounded(theme.radius_sm)
                                        .border_1()
                                        .border_color(theme.border_light)
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(format!(">_ {terminal_count}")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme.space_1)
                            .child(
                                icon_button(&theme, "✎", ("project-edit", id.0 as u64)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.start_editing_project(id, name.clone().into(), cx);
                                    }),
                                ),
                            )
                            .child(
                                icon_button_tinted(
                                    &theme,
                                    "✕",
                                    ("project-archive", id.0 as u64),
                                    theme.danger,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.archive_project(id);
                                            cx.notify();
                                        });
                                    },
                                )),
                            ),
                    );

                let row = if selected {
                    row.bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_light)
                        .shadow_sm()
                } else {
                    row.border_1()
                        .border_color(gpui::transparent_black())
                        .hover(move |s| {
                            s.bg(theme.surface_raised).border_color(theme.border_subtle)
                        })
                };

                row.on_click(cx.listener(move |this, _, _, cx| {
                    // The history view belongs to one project: leave it.
                    this.open_history = None;
                    this.open_task = None;
                    this.open_note = None;
                    this.state.update(cx, |state, _| {
                        state.select_project(id);
                    });
                    cx.notify();
                }))
                .into_any_element()
            }))
            .child(create_row(&theme, &self.new_project_input))
            .when(selected_project.is_some(), |el| {
                let open = self.open_history.is_some();
                el.child(
                    div()
                        .id("open-history")
                        .mt(theme.space_3)
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .px(theme.space_3)
                        .py(theme.space_2)
                        .rounded(theme.radius_md)
                        .cursor_pointer()
                        .border_1()
                        .border_color(if open {
                            theme.border_light
                        } else {
                            gpui::transparent_black()
                        })
                        .hover(move |s| {
                            s.bg(theme.surface_raised).border_color(theme.border_subtle)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx)))
                        .child(div().size(px(6.0)).rounded_full().bg(if open {
                            theme.accent
                        } else {
                            gpui::transparent_black()
                        }))
                        .child(
                            div()
                                .text_color(theme.text_secondary)
                                .child("Commit history"),
                        ),
                )
            })
            .when(selected_project.is_some(), |el| {
                el.child(div().mt(theme.space_1).child(self.run_sidebar_row(cx)))
            })
            .child(
                div()
                    .mt(theme.space_1)
                    .child(self.timesheet_sidebar_row(cx)),
            )
            .when(selected_project.is_some(), |el| {
                el.child(
                    div()
                        .mt(theme.space_1)
                        .child(self.credentials_sidebar_row(cx)),
                )
            });

        let main_panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .bg(theme.surface_solid)
            .p(theme.space_4)
            .gap(theme.space_3)
            .when_some(selected_project_data.as_ref(), |el, project| {
                el.child(dashboard(&theme, project, &raw_tasks, &unreadable_repos))
            })
            .when(selected_project.is_some(), |el| {
                el.child(self.workspace_tools.clone())
            })
            .when(selected_project.is_some() && !notes.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.space_2)
                        .pb(theme.space_3)
                        .border_b_1()
                        .border_color(theme.border_subtle)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child("RECENT NOTES"),
                        )
                        .children(notes.iter().take(5).map(|note: &Note| {
                            let id = note.id;
                            let open_title = note.title.clone();
                            div()
                                .id(("recent-note", id.0 as u64))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(theme.space_2)
                                .px(theme.space_3)
                                .py(theme.space_2)
                                .rounded(theme.radius_md)
                                .bg(theme.surface_raised)
                                .border_1()
                                .border_color(theme.border_subtle)
                                .cursor_pointer()
                                .hover(move |s| s.border_color(theme.border_light))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_note(id, open_title.clone().into(), cx);
                                }))
                                .child(
                                    div()
                                        .text_color(theme.text_primary)
                                        .child(note.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(date::format_date(note.updated_at)),
                                )
                        })),
                )
            })
            .child(div().text_xl().text_color(theme.text_primary).child(
                if selected_project.is_some() {
                    "Tasks"
                } else {
                    "Select or create a project"
                },
            ))
            .when(!project_tags.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(theme.space_1)
                        .child(tag_filter_chip(
                            &theme,
                            "All".to_string(),
                            active_tag.is_none(),
                            "tag-filter-all".into(),
                            cx.listener(|this, _, _, cx| {
                                this.tag_filter = None;
                                cx.notify();
                            }),
                        ))
                        .children(project_tags.iter().enumerate().map(|(ix, (name, count))| {
                            let active = active_tag
                                .as_ref()
                                .is_some_and(|t| t.to_lowercase() == name.to_lowercase());
                            let wanted = name.clone();
                            tag_filter_chip(
                                &theme,
                                format!("{name} · {count}"),
                                active,
                                ("tag-filter", ix as u64).into(),
                                cx.listener(move |this, _, _, cx| {
                                    this.tag_filter = Some(wanted.clone());
                                    cx.notify();
                                }),
                            )
                        })),
                )
            })
            .children(tasks.into_iter().map(|(task, depth)| {
                let id = task.id;

                if editing_task_id == Some(id) {
                    let (_, input) = self.editing_task.as_ref().unwrap();
                    return div()
                        .ml(px(depth as f32 * 20.0))
                        .child(input.clone())
                        .into_any_element();
                }

                let status = task.status;
                let badge_color = match status {
                    TaskStatus::Done => theme.success,
                    TaskStatus::Blocked => theme.warning,
                    TaskStatus::InProgress => theme.accent,
                    TaskStatus::Todo => theme.text_secondary,
                };
                let title = task.title.clone();
                let (note_count, file_count, commit_count) =
                    task_counts.get(&id).copied().unwrap_or_default();
                let counts: Vec<String> = [
                    (note_count, "note"),
                    (file_count, "file"),
                    (commit_count, "commit"),
                ]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, label)| format!("{n} {label}{}", if *n == 1 { "" } else { "s" }))
                .collect();
                div()
                    .ml(px(depth as f32 * 20.0))
                    .id(("task", id.0 as u64))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(theme.space_3)
                    .py(theme.space_3)
                    .rounded(theme.radius_lg)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_subtle)
                    .border_t_1()
                    .shadow_sm()
                    .cursor_pointer()
                    .hover(move |s| s.border_color(theme.border_light))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_3)
                            .child(
                                div()
                                    .text_color(theme.text_primary)
                                    .child(task.title.clone()),
                            )
                            .when(!counts.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(counts.join(" · ")),
                                )
                            })
                            .children(task.tags.iter().take(3).map(|tag| {
                                div()
                                    .px(theme.space_2)
                                    .rounded(theme.radius_sm)
                                    .bg(crate::theme::label_color(tag).opacity(0.16))
                                    .text_xs()
                                    .text_color(crate::theme::label_color(tag))
                                    .child(tag.clone())
                            }))
                            .when(task.tags.len() > 3, |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(format!("+{}", task.tags.len() - 3)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                div()
                                    .id(("task-open", id.0 as u64))
                                    .px(theme.space_2)
                                    .py(theme.space_1)
                                    .rounded(theme.radius_sm)
                                    .border_1()
                                    .border_color(theme.border_light)
                                    .text_sm()
                                    .text_color(theme.text_secondary)
                                    .cursor_pointer()
                                    .hover(move |s| {
                                        s.bg(theme.surface_raised).text_color(theme.text_primary)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_task_detail(id, cx);
                                        cx.stop_propagation();
                                    }))
                                    .child("Open"),
                            )
                            .child(
                                div()
                                    .px(theme.space_2)
                                    .py(theme.space_1)
                                    .rounded(theme.radius_sm)
                                    .bg(badge_color.opacity(0.15))
                                    .text_color(badge_color)
                                    .text_sm()
                                    .child(status_label(status)),
                            )
                            .child(
                                icon_button(&theme, "+", ("task-subtask", id.0 as u64)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        let new_id = this.state.update(cx, |state, cx| {
                                            let new_id = state.create_subtask(id, "New subtask");
                                            cx.notify();
                                            new_id
                                        });
                                        if let Some(new_id) = new_id {
                                            this.start_editing_task(
                                                new_id,
                                                "New subtask".into(),
                                                cx,
                                            );
                                        }
                                    }),
                                ),
                            )
                            .child(
                                icon_button(&theme, "✎", ("task-edit", id.0 as u64)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.start_editing_task(id, title.clone().into(), cx);
                                    }),
                                ),
                            )
                            .child(
                                icon_button_tinted(
                                    &theme,
                                    "✕",
                                    ("task-delete", id.0 as u64),
                                    theme.danger,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.delete_task(id);
                                            cx.notify();
                                        });
                                    },
                                )),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, _| {
                            state.cycle_task_status(id);
                        });
                        cx.notify();
                    }))
                    .into_any_element()
            }))
            .when(selected_project.is_some(), |el| {
                el.child(create_row(&theme, &self.new_task_input))
            })
            .when(selected_project.is_some(), |el| {
                el.child(
                    div()
                        .text_xl()
                        .text_color(theme.text_primary)
                        .mt(theme.space_4)
                        .child("Notes"),
                )
                .children(notes.into_iter().map(|note: Note| {
                    let id = note.id;

                    if editing_note_id == Some(id) {
                        let (_, input) = self.editing_note.as_ref().unwrap();
                        return div().child(input.clone()).into_any_element();
                    }

                    let title = note.title.clone();
                    let open_title = note.title.clone();
                    div()
                        .id(("note", id.0 as u64))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(theme.space_3)
                        .py(theme.space_3)
                        .rounded(theme.radius_lg)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .shadow_sm()
                        .cursor_pointer()
                        .hover(move |s| s.border_color(theme.border_light))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_note(id, open_title.clone().into(), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space_2)
                                .child(
                                    div()
                                        .text_color(theme.text_primary)
                                        .child(note.title.clone()),
                                )
                                .when(note.kind != NoteKind::Note, |d| {
                                    d.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_secondary)
                                            .child(note.kind.label()),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space_2)
                                .child(
                                    icon_button(&theme, "✎", ("note-edit", id.0 as u64)).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.start_editing_note(id, title.clone().into(), cx);
                                        }),
                                    ),
                                )
                                .child(
                                    icon_button_tinted(
                                        &theme,
                                        "✕",
                                        ("note-delete", id.0 as u64),
                                        theme.danger,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.delete_note(id);
                                                cx.notify();
                                            });
                                        },
                                    )),
                                ),
                        )
                        .into_any_element()
                }))
                .child(
                    div()
                        .mt(theme.space_2)
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .child(
                            div()
                                .id("note-kind")
                                .flex_none()
                                .px(theme.space_3)
                                .py(theme.space_1)
                                .rounded(theme.radius_sm)
                                .border_1()
                                .border_color(theme.accent)
                                .bg(theme.accent.opacity(0.12))
                                .text_sm()
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.new_note_kind = state.new_note_kind.cycle();
                                        cx.notify();
                                    });
                                }))
                                .child(format!("Kind: {}", new_note_kind.label())),
                        )
                        .child(div().flex_1().child(self.new_note_input.clone())),
                )
                .child(
                    div()
                        .px(theme.space_1)
                        .text_color(theme.text_secondary)
                        .text_xs()
                        .child("Click the kind to change it. Enter creates, Esc cancels"),
                )
            })
            .when(selected_project.is_some(), |el| {
                el.child(self.project_files.clone())
            })
            .when(selected_project.is_some(), |el| {
                el.child(
                    div()
                        .text_xl()
                        .text_color(theme.text_primary)
                        .mt(theme.space_4)
                        .child("Linked repos"),
                )
                .children(repos.into_iter().map(|repo: Repo| {
                    let id = repo.id;
                    let link_count = repo_link_counts.get(&id).copied().unwrap_or(0);
                    let confirming = confirm_unlink == Some(id);
                    div()
                        .id(("repo", id.0 as u64))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(theme.space_3)
                        .py(theme.space_3)
                        .rounded(theme.radius_lg)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .shadow_sm()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_color(theme.text_primary)
                                        .child(repo.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(repo.path.clone()),
                                )
                                .when(confirming, |d| {
                                    d.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(theme.space_2)
                                            .text_xs()
                                            .child(div().text_color(theme.warning).child(format!(
                                                "{link_count} commit mapping{} will be deleted \
                                                 with this repo. Click ✕ again to confirm.",
                                                if link_count == 1 { "" } else { "s" }
                                            )))
                                            .child(
                                                div()
                                                    .id(("repo-unlink-cancel", id.0 as u64))
                                                    .text_color(theme.text_secondary)
                                                    .cursor_pointer()
                                                    .hover(move |s| {
                                                        s.text_color(theme.text_primary)
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.confirm_unlink_repo = None;
                                                        cx.notify();
                                                    }))
                                                    .child("Cancel"),
                                            ),
                                    )
                                }),
                        )
                        .child(
                            icon_button_tinted(
                                &theme,
                                "✕",
                                ("repo-unlink", id.0 as u64),
                                theme.danger,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    // Mapped commits go with the repo (the
                                    // database cascades); ask first.
                                    if link_count > 0 && this.confirm_unlink_repo != Some(id) {
                                        this.confirm_unlink_repo = Some(id);
                                        cx.notify();
                                        return;
                                    }
                                    this.confirm_unlink_repo = None;
                                    this.state.update(cx, |state, cx| {
                                        state.unlink_repo(id);
                                        cx.notify();
                                    });
                                },
                            )),
                        )
                        .into_any_element()
                }))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(theme.space_2)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(create_row(&theme, &self.new_repo_input)),
                        )
                        .child(
                            div()
                                .id("browse-repo")
                                .mt(theme.space_2)
                                .flex_none()
                                .px(theme.space_4)
                                .py(theme.space_2)
                                .rounded(theme.radius_md)
                                .border_1()
                                .border_color(theme.accent)
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .hover(move |s| s.bg(theme.surface_raised))
                                .on_click(cx.listener(|this, _, _, cx| this.browse_repo(cx)))
                                .child("Browse…"),
                        ),
                )
            });

        let mut note_page =
            (self.open_note.is_some() && self.open_history.is_none()).then(|| self.note_page(cx));
        let main_panel = if let Some(history) = &self.open_history {
            div()
                .flex()
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .child(history.clone())
                .into_any_element()
        } else if let (Some(detail), None) = (&self.open_task, &self.open_note) {
            div()
                .flex()
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .child(detail.clone())
                .into_any_element()
        } else if let Some(page) = note_page.take() {
            page
        } else if self.run_open {
            div()
                .flex()
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .child(self.run_panel.clone())
                .into_any_element()
        } else if self.timesheet_open {
            div()
                .flex()
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .child(self.timesheet_panel.clone())
                .into_any_element()
        } else if self.credentials_open {
            div()
                .flex()
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .child(self.credentials_panel.clone())
                .into_any_element()
        } else {
            main_panel.into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.window_tint)
            .font_family(theme::UI_FONT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                this.toggle_terminal(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                this.terminal_open = true;
                this.terminal_panel
                    .update(cx, |panel, cx| panel.new_tab(window, cx));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .child(titlebar(&theme))
            .child(
                div().flex().flex_col().flex_1().child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(sidebar)
                        .child(main_panel)
                        .when(self.terminal_open, |el| {
                            el.child(self.terminal_panel.clone())
                        }),
                ),
            )
            .children(self.palette.clone())
    }
}

fn init_data() -> Result<(WorkspacePaths, rusqlite::Connection)> {
    let paths = WorkspacePaths::from_os_data_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve OS data directory"))?;
    paths.ensure_dirs(None)?;

    let mut conn = devspace_storage::open_connection(&paths.database_file())?;
    run_migrations(&mut conn)?;
    Ok((paths, conn))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let (paths, conn) = init_data()?;

    gpui_platform::application().run(move |cx: &mut App| {
        text_input::init_key_bindings(cx);
        note_editor::init_key_bindings(cx);
        // Bound globally (no key context) so it also fires while a terminal
        // has focus; GPUI matches bindings before the terminal's raw
        // key-down handler sees the keystroke.
        // Same as VS Code: Ctrl+` toggles the panel, Ctrl+Shift+` opens a
        // new terminal.
        cx.bind_keys([
            KeyBinding::new("ctrl-`", ToggleTerminal, None),
            KeyBinding::new("ctrl-shift-`", NewTerminal, None),
            KeyBinding::new("ctrl-k", OpenPalette, None),
            KeyBinding::new("ctrl-shift-p", OpenPalette, None),
        ]);

        let bounds = Bounds::centered(None, size(px(1000.0), px(640.0)), cx);
        let state = cx.new(|_| AppState::new(conn, paths.clone()));
        let theme = GlassTheme::dark();
        // Restore last project / task / terminal visibility. Stale ids fall
        // back to the defaults silently (see settings::restore_selection).
        let settings = cx.new(|_| SettingsStore::load(settings::settings_path(paths.root())));
        let saved = settings.read(cx).get().clone();
        let restored_task = state.update(cx, |state, _| settings::restore_selection(state, &saved));

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Devspace".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_background: WindowBackgroundAppearance::MicaBackdrop,
                ..Default::default()
            },
            |window, cx| {
                let new_project_input = cx.new(|cx| TextInput::new(cx, "New project name…", theme));
                let new_task_input = cx.new(|cx| TextInput::new(cx, "New task title…", theme));
                let new_note_input = cx.new(|cx| TextInput::new(cx, "New note title…", theme));
                let new_repo_input =
                    cx.new(|cx| TextInput::new(cx, "Repo path (e.g. C:\\code\\myrepo)…", theme));

                let project_state = state.clone();
                new_project_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |name, _window, cx| {
                        let name = name.trim().to_string();
                        if name.is_empty() {
                            return;
                        }
                        project_state.update(cx, |state, cx| {
                            state.create_project(&name);
                            cx.notify();
                        });
                    }));
                });

                let task_state = state.clone();
                new_task_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |title, _window, cx| {
                        let title = title.trim().to_string();
                        if title.is_empty() {
                            return;
                        }
                        task_state.update(cx, |state, cx| {
                            state.create_task(&title);
                            cx.notify();
                        });
                    }));
                });

                let note_state = state.clone();
                new_note_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |title, _window, cx| {
                        let title = title.trim().to_string();
                        if title.is_empty() {
                            return;
                        }
                        note_state.update(cx, |state, cx| {
                            state.create_note(&title);
                            cx.notify();
                        });
                    }));
                });

                let repo_state = state.clone();
                new_repo_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |path, _window, cx| {
                        let path = path.trim().to_string();
                        if path.is_empty() {
                            return;
                        }
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        repo_state.update(cx, |state, cx| {
                            state.link_repo(&path, &name);
                            cx.notify();
                        });
                    }));
                });

                cx.new(|cx| {
                    cx.observe(&state, |_, _, cx| cx.notify()).detach();
                    let terminal_panel = cx.new(|cx| TerminalPanel::new(cx, state.clone(), theme));
                    // Re-render the sidebar's per-project terminal badges.
                    cx.observe(&terminal_panel, |_, _, cx| cx.notify()).detach();
                    let root: Entity<Root> = cx.entity();
                    let run_panel = cx.new(|cx| {
                        let mut panel = RunPanel::new(cx, state.clone(), theme);
                        panel.on_close = Some(Box::new(move |_window, cx| {
                            root.update(cx, |root, cx| {
                                root.run_open = false;
                                cx.notify();
                            });
                        }));
                        panel
                    });
                    // Re-render the sidebar's running-commands badge.
                    cx.observe(&run_panel, |_, _, cx| cx.notify()).detach();
                    let root_for_timesheet: Entity<Root> = cx.entity();
                    let timesheet_panel = cx.new(|cx| {
                        let mut panel = TimesheetPanel::new(cx, state.clone(), theme);
                        panel.on_close = Some(Box::new(move |_window, cx| {
                            root_for_timesheet.update(cx, |root, cx| {
                                root.timesheet_open = false;
                                cx.notify();
                            });
                        }));
                        panel
                    });
                    let root_for_credentials: Entity<Root> = cx.entity();
                    let credentials_panel = cx.new(|cx| {
                        let mut panel = CredentialsPanel::new(cx, state.clone(), theme);
                        panel.on_close = Some(Box::new(move |_window, cx| {
                            root_for_credentials.update(cx, |root, cx| {
                                root.credentials_open = false;
                                cx.notify();
                            });
                        }));
                        panel
                    });
                    // Global shortcuts (toggle terminal, ...) are actions
                    // dispatched along the focused element's ancestors, so
                    // the root needs focus whenever nothing else has it.
                    let focus_handle = cx.focus_handle();
                    window.focus(&focus_handle, cx);
                    let workspace_tools = cx
                        .new(|cx| WorkspaceTools::new(cx, state.clone(), settings.clone(), theme));
                    let project_files = cx.new(|_| ProjectFiles::new(state.clone(), theme));
                    let mut root = Root {
                        settings: settings.clone(),
                        workspace_tools,
                        project_files,
                        state,
                        theme,
                        new_project_input,
                        new_task_input,
                        new_note_input,
                        new_repo_input,
                        editing_project: None,
                        editing_task: None,
                        editing_note: None,
                        open_note: None,
                        note_mode: note_page::NoteMode::Edit,
                        note_preview: None,
                        terminal_panel,
                        terminal_open: saved.terminal_open,
                        focus_handle,
                        open_history: None,
                        open_task: None,
                        confirm_unlink_repo: None,
                        tag_filter: None,
                        palette: None,
                        palette_prev_focus: None,
                        run_panel,
                        run_open: false,
                        timesheet_panel,
                        timesheet_open: false,
                        credentials_panel,
                        credentials_open: false,
                    };
                    // A visible terminal panel needs its first tab, exactly
                    // as when the user opens it; a hidden one spawns nothing.
                    if root.terminal_open {
                        root.terminal_panel
                            .update(cx, |panel, cx| panel.ensure_tab(window, cx));
                    }
                    if let Some(task) = restored_task {
                        root.open_task_detail(task, cx);
                    }
                    root
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });

    Ok(())
}
