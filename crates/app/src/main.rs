//! GPUI entry point and root view.
//! Pin: zed-industries/zed @ 56934ec0560bce8929025497411de7f5322fb6ca
//! (see CLAUDE.md "GPUI guidelines" — pin an exact revision, don't guess
//! the pre-1.0 API).

// Release builds are a GUI app on Windows: no console window behind them.
// Debug builds keep the console so tracing output stays visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod attach_flow;
mod changes_model;
mod changes_view;
mod clipboard_html;
mod commit_browser;
mod credentials_panel;
mod date;
mod external;
mod highlight;
mod ide;
mod markdown;
mod markdown_view;
mod motion;
mod note_editor;
mod note_format;
mod note_page;
mod palette;
mod palette_rank;
mod project_files;
mod project_page;
mod qa;
mod run_panel;
mod settings;
mod shell;
mod state;
mod task_detail;
mod task_tree;
mod terminal_panel;
mod terminal_view;
mod text_input;
mod text_nav;
mod theme;
mod timesheet_panel;
mod tips;
mod ui;
mod workspace_tools;

use crate::theme::TypeScale;
use anyhow::Result;
use changes_view::ChangesView;
use commit_browser::CommitBrowser;
use credentials_panel::CredentialsPanel;
use devspace_core::{NoteId, ProjectId, Task, TaskId, TaskStatus};
use devspace_storage::{run_migrations, WorkspacePaths};
use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Entity, KeyBinding,
    PathPromptOptions, SharedString, TitlebarOptions, Window, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowOptions,
};
use note_editor::NoteEditor;
use project_files::ProjectFiles;
use run_panel::RunPanel;
use settings::SettingsStore;
use state::AppState;
use task_detail::TaskDetail;
use terminal_panel::TerminalPanel;
use text_input::TextInput;
use theme::{ActiveTheme, GlassTheme};
use timesheet_panel::TimesheetPanel;
use workspace_tools::WorkspaceTools;

actions!(
    devspace,
    [
        ToggleTerminal,
        NewTerminal,
        OpenPalette,
        ToggleHistoryView,
        ToggleChangesView,
        ToggleRunView,
        ToggleTimesheetView,
        ToggleCredentialsView,
        ToggleSettingsView,
        FocusNewProject
    ]
);

struct Root {
    state: Entity<AppState>,
    theme: GlassTheme,
    /// Window backdrop currently applied (blurred, or opaque when transparency
    /// is reduced).
    background: WindowBackgroundAppearance,
    /// Key of the hover-animated row under the pointer (see `set_hover`).
    hovered: Option<u64>,
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
    /// Code block whose copy button was just pressed (shows "Copied" briefly).
    note_copied: Option<usize>,
    /// Right-hand terminal panel (CLAUDE.md layout concept: "collapsible
    /// panels for terminal, commit browser and credentials"). Owns a tab set
    /// per project; hiding it or switching projects keeps shells running.
    terminal_panel: Entity<TerminalPanel>,
    focus_handle: gpui::FocusHandle,
    /// The commit history browser, replacing the main content while open
    /// (like `open_note`).
    open_history: Option<Entity<CommitBrowser>>,
    /// The git Changes view (working copy of every linked repo), replacing the
    /// main content while open.
    open_changes: Option<Entity<ChangesView>>,
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
    /// Settings view (appearance, terminal, IDE, backup), shown in place of the
    /// main content.
    settings_open: bool,
    /// The toast currently on screen (mirrors the workspace tools' notice) and a
    /// counter so only the newest toast's timer dismisses it.
    toast_seen: Option<(String, bool)>,
    toast_epoch: u64,
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

/// A fixed sidebar navigation row: its element id, its key for hover tracking
/// (project rows add their id to [`PROJECT_HOVER_KEY_BASE`], so the ranges
/// never collide), its icon, label and keyboard shortcut.
#[derive(Clone, Copy)]
pub(crate) struct NavItem {
    pub id: &'static str,
    pub hover_key: u64,
    pub icon: ui::Icon,
    pub label: &'static str,
    pub shortcut: &'static str,
}

pub(crate) const NAV_HISTORY: NavItem = NavItem {
    id: "open-history",
    hover_key: 1,
    icon: ui::Icon::GitCommit,
    label: "Commit history",
    shortcut: "Ctrl+Shift+H",
};
pub(crate) const NAV_CHANGES: NavItem = NavItem {
    id: "open-changes",
    hover_key: 6,
    icon: ui::Icon::GitMerge,
    label: "Changes",
    shortcut: "Ctrl+Shift+G",
};
pub(crate) const NAV_RUN: NavItem = NavItem {
    id: "open-run-commands",
    hover_key: 2,
    icon: ui::Icon::Play,
    label: "Run commands",
    shortcut: "Ctrl+Shift+R",
};
pub(crate) const NAV_TIMESHEET: NavItem = NavItem {
    id: "open-timesheet",
    hover_key: 3,
    icon: ui::Icon::Clock,
    label: "Timesheet",
    shortcut: "Ctrl+Shift+M",
};
pub(crate) const NAV_CREDENTIALS: NavItem = NavItem {
    id: "open-credentials",
    hover_key: 4,
    icon: ui::Icon::Key,
    label: "Credentials",
    shortcut: "Ctrl+Shift+L",
};
pub(crate) const NAV_SETTINGS: NavItem = NavItem {
    id: "open-settings",
    hover_key: 5,
    icon: ui::Icon::Settings,
    label: "Settings",
    shortcut: "Ctrl+,",
};
pub(crate) const PROJECT_HOVER_KEY_BASE: u64 = 1_000_000;

/// Whether the OS is in a dark appearance.
fn os_is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// Window backdrop for a transparency setting: blurred normally, opaque when
/// the user asked to reduce transparency.
fn background_for(reduce_transparency: bool) -> WindowBackgroundAppearance {
    if reduce_transparency {
        WindowBackgroundAppearance::Opaque
    } else {
        WindowBackgroundAppearance::MicaBackdrop
    }
}

/// Compatibility wrappers: older call sites pass a text glyph (pencil, cross,
/// arrow); it is mapped onto the line-icon set so no screen mixes glyphs and
/// icons. New code calls [`ui::icon_button`] with an [`ui::Icon`] directly.
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
    ui::icon_button(
        theme,
        ui::Icon::from_glyph(glyph).unwrap_or(ui::Icon::Info),
        id,
        hover_color,
    )
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

    /// Opens the OS folder picker and stores the chosen folder as the
    /// selected project's working folder (what "Open in IDE" opens).
    fn browse_project_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Project folder".into()),
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
            let path = chosen.to_string_lossy().to_string();
            state.update(cx, |state, cx| {
                state.set_project_folder(Some(&path));
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
        self.settings_open = false;
        self.open_task = None;
        self.run_open = false;
        self.timesheet_open = false;
        self.credentials_open = false;
        self.open_changes = None;
        self.open_history = Some(browser);
        cx.notify();
    }

    /// Opens the git Changes view in place of the main content, or closes it
    /// if already open.
    pub(crate) fn toggle_changes(&mut self, cx: &mut Context<Self>) {
        if self.open_changes.take().is_some() {
            cx.notify();
            return;
        }
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let settings = self.settings.clone();
        let view = cx.new(|cx| {
            let mut view = ChangesView::new(cx, state, settings, theme);
            view.on_close = Some(Box::new(move |_window, cx| {
                root.update(cx, |root, cx| {
                    root.open_changes = None;
                    cx.notify();
                });
            }));
            view
        });
        self.open_note = None;
        self.settings_open = false;
        self.open_task = None;
        self.run_open = false;
        self.timesheet_open = false;
        self.credentials_open = false;
        self.open_history = None;
        self.open_changes = Some(view);
        cx.notify();
    }

    /// A sidebar navigation row (icon, label, optional badge, shortcut hint on
    /// hover) whose surface fades in while it is open or hovered. `hover_key`
    /// identifies it in [`Root::set_hover`]; `id` names the element and its
    /// transition.
    pub(crate) fn nav_row(
        &self,
        item: NavItem,
        open: bool,
        badge: Option<gpui::AnyElement>,
        on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let NavItem {
            id,
            hover_key,
            icon,
            label,
            shortcut,
        } = item;
        let theme = self.theme;
        let hovered = self.hovered == Some(hover_key);
        let row = div()
            .id(id)
            .flex()
            .items_center()
            .gap(theme.space_2)
            .h(px(32.0))
            .pl(theme.space_3)
            .pr(theme.space_2)
            .rounded(theme.radius_md)
            .cursor_pointer()
            .border_1()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_hover(hover_key, *hovered, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(ui::icon(
                icon,
                15.0,
                if open { theme.accent } else { theme.text_muted },
            ))
            .child(
                div()
                    .flex_1()
                    .t_body()
                    .text_color(if open {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    })
                    .when(open, |d| d.t_strong())
                    .child(label),
            )
            .children(badge)
            .when(hovered && !open && !shortcut.is_empty(), |d| {
                d.child(
                    div()
                        .t_micro()
                        .t_code()
                        .text_color(theme.text_muted)
                        .child(shortcut),
                )
            });
        let (bg, border) = if open {
            (theme.accent.opacity(0.10), theme.accent.opacity(0.22))
        } else {
            (theme.surface_hover, gpui::transparent_black())
        };
        motion::fade(
            row,
            gpui::ElementId::Name(format!("{id}-fade").into()),
            open || hovered,
            bg,
            border,
        )
    }

    /// Records that the pointer entered or left the row with this key, so its
    /// hover transition can run. Only a real change re-renders.
    pub(crate) fn set_hover(&mut self, key: u64, hovered: bool, cx: &mut Context<Self>) {
        let next = if hovered {
            Some(key)
        } else if self.hovered == Some(key) {
            None
        } else {
            self.hovered
        };
        if next != self.hovered {
            self.hovered = next;
            cx.notify();
        }
    }

    /// Applies the theme settings: picks the palette (following the OS for
    /// "Follow system"), switches the window backdrop between blurred and
    /// opaque, and redraws every window when anything changed.
    fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.read(cx).get().clone();
        let wanted = GlassTheme::resolve(
            settings.theme_mode,
            os_is_dark(window.appearance()),
            settings.reduce_transparency,
        );
        let background = background_for(settings.reduce_transparency);
        if background != self.background {
            window.set_background_appearance(background);
            self.background = background;
        }
        cx.set_reduce_motion(settings.reduce_motion);
        if theme::current(cx) != wanted {
            cx.set_global(ActiveTheme(wanted));
            cx.refresh_windows();
        }
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

    /// Opens a new terminal tab in `dir`, showing the panel if it is hidden.
    /// A folder that no longer exists gets a toast, not a shell elsewhere.
    fn open_terminal_in(
        &mut self,
        dir: std::path::PathBuf,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !dir.is_dir() {
            let message = format!("Folder not found: {}", dir.display());
            self.workspace_tools
                .update(cx, |tools, cx| tools.set_notice(message, true, cx));
            return;
        }
        self.terminal_open = true;
        self.terminal_panel.update(cx, |panel, cx| {
            panel.new_tab_in(Some(dir), Some(label), window, cx)
        });
        cx.notify();
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        self.sync_settings(cx);
        let theme = self.theme;
        let titlebar = self.titlebar(cx);
        let sidebar = self.sidebar(cx);

        /// A full-content view fills the canvas and clips to it, so a narrow
        /// window cuts it off instead of painting over the terminal panel.
        fn fill(view: impl IntoElement) -> gpui::AnyElement {
            div()
                .flex()
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(view)
                .into_any_element()
        }

        let mut note_page = (self.open_note.is_some()
            && self.open_history.is_none()
            && self.open_changes.is_none())
        .then(|| self.note_page(cx));
        let view = if let Some(history) = &self.open_history {
            fill(history.clone())
        } else if let Some(changes) = &self.open_changes {
            fill(changes.clone())
        } else if let (Some(detail), None) = (&self.open_task, &self.open_note) {
            fill(detail.clone())
        } else if let Some(page) = note_page.take() {
            page
        } else if self.run_open {
            fill(self.run_panel.clone())
        } else if self.timesheet_open {
            fill(self.timesheet_panel.clone())
        } else if self.credentials_open {
            fill(self.credentials_panel.clone())
        } else if self.settings_open {
            fill(self.workspace_tools.clone())
        } else {
            self.project_page(window, cx)
        };
        let terminal = self
            .terminal_open
            .then(|| self.terminal_panel.clone().into_any_element());
        let canvas = self.canvas(view, terminal);
        let toast = self.toast(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.window_tint)
            .font_family(theme::UI_FONT)
            .text_size(px(theme::FS_BODY))
            .line_height(gpui::relative(theme::LINE_HEIGHT))
            .text_color(theme.text_primary)
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
            // Dragging the terminal panel's left edge resizes it; the pointer
            // can leave the 6px handle, so the drag is tracked up here.
            .on_drag_move::<terminal_panel::DraggedTerminalEdge>(cx.listener(
                |this,
                 event: &gpui::DragMoveEvent<terminal_panel::DraggedTerminalEdge>,
                 window,
                 cx| {
                    let width = terminal_panel::width_for_pointer(
                        window.viewport_size().width.into(),
                        event.event.position.x.into(),
                    );
                    this.terminal_panel
                        .update(cx, |panel, cx| panel.set_width(width, cx));
                },
            ))
            .on_action(cx.listener(|this, _: &ToggleHistoryView, _, cx| this.toggle_history(cx)))
            .on_action(cx.listener(|this, _: &ToggleChangesView, _, cx| this.toggle_changes(cx)))
            .on_action(cx.listener(|this, _: &ToggleRunView, _, cx| this.toggle_run_view(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTimesheetView, _, cx| this.toggle_timesheet_view(cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleCredentialsView, _, cx| {
                this.toggle_credentials_view(cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleSettingsView, _, cx| this.toggle_settings_view(cx)),
            )
            .on_action(cx.listener(|this, _: &FocusNewProject, window, cx| {
                this.run_command(palette_rank::Command::NewProject, window, cx)
            }))
            .child(titlebar)
            .child(
                // min_h(0): without it this column grows with its tallest
                // child and pushes the bottom of the window off-screen.
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(sidebar)
                    .child(canvas),
            )
            .children(self.palette.clone())
            .children(toast)
    }
}

fn init_data() -> Result<(WorkspacePaths, rusqlite::Connection)> {
    let paths = qa::data_dir()
        .map(WorkspacePaths::new)
        .or_else(WorkspacePaths::from_os_data_dir)
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
        theme::install_fonts(cx);
        text_input::init_key_bindings(cx);
        note_editor::init_key_bindings(cx);
        // Bound globally (no key context) so it also fires while a terminal
        // has focus; GPUI matches bindings before the terminal's raw
        // key-down handler sees the keystroke.
        // Same as VS Code: Ctrl+` toggles the panel, Ctrl+Shift+` opens a
        // new terminal. The Ctrl+Shift letters open (and again close) the main views.
        cx.bind_keys([
            KeyBinding::new("ctrl-`", ToggleTerminal, None),
            KeyBinding::new("ctrl-shift-`", NewTerminal, None),
            KeyBinding::new("ctrl-k", OpenPalette, None),
            KeyBinding::new("ctrl-shift-p", OpenPalette, None),
            KeyBinding::new("ctrl-shift-h", ToggleHistoryView, None),
            KeyBinding::new("ctrl-shift-g", ToggleChangesView, None),
            KeyBinding::new("ctrl-shift-r", ToggleRunView, None),
            KeyBinding::new("ctrl-shift-m", ToggleTimesheetView, None),
            KeyBinding::new("ctrl-shift-l", ToggleCredentialsView, None),
            KeyBinding::new("ctrl-shift-n", FocusNewProject, None),
            KeyBinding::new("ctrl-,", ToggleSettingsView, None),
        ]);

        let (width, height) = qa::window_size().unwrap_or((1000.0, 640.0));
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        let state = cx.new(|_| AppState::new(conn, paths.clone()));

        // Restore last project / task / terminal visibility. Stale ids fall
        // back to the defaults silently (see settings::restore_selection).
        let settings = cx.new(|_| SettingsStore::load(settings::settings_path(paths.root())));
        let saved = settings.read(cx).get().clone();
        // Follow the OS (or the saved choice) from the first frame.
        let theme = GlassTheme::resolve(
            saved.theme_mode,
            os_is_dark(cx.window_appearance()),
            saved.reduce_transparency,
        );
        cx.set_global(ActiveTheme(theme));
        cx.set_reduce_motion(saved.reduce_motion);
        let background = background_for(saved.reduce_transparency);
        let restored_task = state.update(cx, |state, _| settings::restore_selection(state, &saved));

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Devspace".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_background: background,
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
                    let terminal_panel =
                        cx.new(|cx| TerminalPanel::new(cx, state.clone(), settings.clone(), theme));
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
                        let mut panel =
                            TimesheetPanel::new(cx, state.clone(), settings.clone(), theme);
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
                        background,
                        hovered: None,
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
                        note_copied: None,
                        note_preview: None,
                        terminal_panel,
                        terminal_open: saved.terminal_open,
                        focus_handle,
                        open_history: None,
                        open_changes: None,
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
                        settings_open: false,
                        toast_seen: None,
                        toast_epoch: 0,
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
                    qa::open_view(&mut root, window, cx);
                    // Restyle at runtime when the theme or transparency setting
                    // changes, or the OS switches between light and dark.
                    cx.observe_in(&settings, window, |this, _, window, cx| {
                        this.apply_appearance(window, cx);
                    })
                    .detach();
                    cx.observe_window_appearance(window, |this, window, cx| {
                        this.apply_appearance(window, cx);
                    })
                    .detach();
                    // The Changes view re-reads git when the window regains
                    // focus (edits made in the IDE or a terminal meanwhile).
                    cx.observe_window_activation(window, |this, window, cx| {
                        if window.is_window_active() {
                            if let Some(changes) = this.open_changes.clone() {
                                changes.update(cx, |changes, cx| changes.refresh_all(cx));
                            }
                        }
                    })
                    .detach();
                    root
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });

    Ok(())
}
