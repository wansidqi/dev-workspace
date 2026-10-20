//! The "Workspace tools" card on the project page: open the project in the
//! user's IDE (with an editable command) and back up / export / import.
//! Slow work (copying files, SQLite) runs on a background task with its own
//! database connection so the UI thread never blocks; the result shows as a
//! notice.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use devspace_core::ProjectId;
use gpui::{div, prelude::*, px, Context, Entity, PathPromptOptions, SharedString, Window};

use crate::ide;
use crate::motion;
use crate::settings::{parse_scrollback, SettingsStore};
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::{GlassTheme, ThemeMode, TypeScale};
use crate::ui::{button, page_header, section_label, segment, segmented, switch, ButtonKind, Icon};

/// How long a background connection waits for the app's own connection to
/// release the database before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// What a finished background job reports.
struct JobDone {
    message: String,
    /// A project the job added to the database; the app should select it.
    imported: Option<ProjectId>,
}

pub struct WorkspaceTools {
    state: Entity<AppState>,
    settings: Entity<SettingsStore>,
    theme: GlassTheme,
    ide_input: Entity<TextInput>,
    scrollback_input: Entity<TextInput>,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    /// A backup/export/import is running; further ones are refused.
    busy: bool,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A second, short-lived connection for background jobs. WAL mode lets it
/// read while the app's connection is open; the busy timeout covers writes.
fn job_connection(
    paths: &devspace_storage::WorkspacePaths,
) -> Result<rusqlite::Connection, String> {
    let conn = devspace_storage::open_connection(&paths.database_file())
        .map_err(|e| format!("Couldn't open the database: {e}"))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|e| format!("Couldn't open the database: {e}"))?;
    Ok(conn)
}

impl WorkspaceTools {
    pub fn new(
        cx: &mut Context<Self>,
        state: Entity<AppState>,
        settings: Entity<SettingsStore>,
        theme: GlassTheme,
    ) -> Self {
        let ide_input = cx.new(|cx| TextInput::new(cx, "e.g. code --reuse-window", theme));
        let this = cx.entity();
        ide_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |command, _window, cx| {
                let command = command.trim().to_string();
                if command.is_empty() {
                    return;
                }
                this.update(cx, |tools, cx| tools.save_ide_command(&command, cx));
            }));
        });
        let scrollback_input = cx.new(|cx| TextInput::new(cx, "e.g. 5000", theme));
        let this = cx.entity();
        scrollback_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                this.update(cx, |tools, cx| tools.save_scrollback(&text, cx));
            }));
        });
        Self {
            state,
            settings,
            theme,
            ide_input,
            scrollback_input,
            notice: None,
            busy: false,
        }
    }

    /// The latest result message and whether it is an error, shown as a toast.
    pub(crate) fn notice(&self) -> Option<&(String, bool)> {
        self.notice.as_ref()
    }

    /// Shows `message` as a toast (an error when `is_error`).
    pub(crate) fn set_notice(&mut self, message: String, is_error: bool, cx: &mut Context<Self>) {
        self.notice = Some((message, is_error));
        cx.notify();
    }

    /// Dismisses the toast.
    pub(crate) fn clear_notice(&mut self, cx: &mut Context<Self>) {
        if self.notice.take().is_some() {
            cx.notify();
        }
    }

    fn save_ide_command(&mut self, command: &str, cx: &mut Context<Self>) {
        self.notice = Some(match ide::split_command(command) {
            Ok(_) => {
                let command = command.to_string();
                self.settings.update(cx, |store, cx| {
                    store.update(cx, |s| s.ide_command = command)
                });
                ("IDE command saved".to_string(), false)
            }
            Err(e) => (e.to_string(), true),
        });
        cx.notify();
    }

    /// Saves the scrollback length used by terminals opened from now on
    /// (a running terminal keeps the history size it started with).
    fn save_scrollback(&mut self, text: &str, cx: &mut Context<Self>) {
        self.notice = Some(match parse_scrollback(text) {
            Ok(lines) => {
                self.settings.update(cx, |store, cx| {
                    store.update(cx, |s| s.terminal_scrollback = lines)
                });
                (
                    format!("Scrollback set to {lines} lines for new terminals"),
                    false,
                )
            }
            Err(message) => (message, true),
        });
        cx.notify();
    }

    fn set_theme(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |store, cx| store.update(cx, |s| s.theme_mode = mode));
        cx.notify();
    }

    fn toggle_reduce_transparency(&mut self, cx: &mut Context<Self>) {
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.reduce_transparency = !s.reduce_transparency)
        });
        cx.notify();
    }

    fn toggle_reduce_motion(&mut self, cx: &mut Context<Self>) {
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.reduce_motion = !s.reduce_motion)
        });
        cx.notify();
    }

    pub(crate) fn open_in_ide(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.state.read(cx).project_folder() else {
            self.notice = Some(("Select a project first".to_string(), true));
            cx.notify();
            return;
        };
        let command = self.settings.read(cx).get().ide_command.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ide::open_in_ide(&command, &folder) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok(()) => ("Opening in the IDE…".to_string(), false),
                    Err(message) => (message, true),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs `job` on a background thread and shows its outcome.
    fn run_job(
        &mut self,
        cx: &mut Context<Self>,
        running: &str,
        job: impl FnOnce() -> Result<JobDone, String> + Send + 'static,
    ) {
        if self.busy {
            self.notice = Some(("Another backup, export or import is running".into(), true));
            cx.notify();
            return;
        }
        self.busy = true;
        self.notice = Some((running.to_string(), false));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { job() }).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.notice = Some(match result {
                    Ok(done) => {
                        if let Some(project) = done.imported {
                            this.state.update(cx, |state, cx| {
                                state.adopt_imported_project(project);
                                cx.notify();
                            });
                        }
                        (done.message, false)
                    }
                    Err(message) => (message, true),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows the SQLite database file in the file manager. Notes and attachments
    /// live in the folders next to it.
    fn reveal_database(&mut self, cx: &mut Context<Self>) {
        let db = self.state.read(cx).workspace_paths().database_file();
        self.notice = Some(match crate::external::reveal(&db) {
            Ok(()) => ("Opened the data folder".to_string(), false),
            Err(message) => (message, true),
        });
        cx.notify();
    }

    fn backup(&mut self, cx: &mut Context<Self>) {
        let paths = self.state.read(cx).workspace_paths().clone();
        self.run_job(cx, "Backing up the workspace…", move || {
            let conn = job_connection(&paths)?;
            let report = devspace_storage::backup_workspace(&conn, &paths, now_secs())
                .map_err(|e| format!("Backup failed: {e}"))?;
            Ok(JobDone {
                message: format!(
                    "Backed up the database and {} files to {}",
                    report.files,
                    report.dir.display()
                ),
                imported: None,
            })
        });
    }

    /// Asks for a folder, then calls `then` with it (unless cancelled).
    fn pick_folder(
        &mut self,
        prompt: &'static str,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, PathBuf, &mut Context<Self>) + 'static,
    ) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        cx.spawn(async move |this, cx| {
            // Err = picker unavailable, Ok(None) = cancelled.
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| then(this, folder, cx));
        })
        .detach();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.state.read(cx).selected_project else {
            return;
        };
        let paths = self.state.read(cx).workspace_paths().clone();
        self.pick_folder("Export here", cx, move |this, parent, cx| {
            this.run_job(cx, "Exporting the project…", move || {
                let conn = job_connection(&paths)?;
                let report = devspace_storage::export_project(&conn, &paths, project, &parent)
                    .map_err(|e| format!("Export failed: {e}"))?;
                let missing = if report.missing_files > 0 {
                    format!(" ({} files were missing and skipped)", report.missing_files)
                } else {
                    String::new()
                };
                Ok(JobDone {
                    message: format!(
                        "Exported {} files to {}{missing}",
                        report.files,
                        report.dir.display()
                    ),
                    imported: None,
                })
            });
        });
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let paths = self.state.read(cx).workspace_paths().clone();
        self.pick_folder("Import this export folder", cx, move |this, folder, cx| {
            this.run_job(cx, "Importing the project…", move || {
                let mut conn = job_connection(&paths)?;
                let report = devspace_storage::import_project(&mut conn, &paths, &folder)
                    .map_err(|e| format!("Import failed: {e}"))?;
                let skipped = if report.skipped.is_empty() {
                    String::new()
                } else {
                    format!(" ({} items skipped)", report.skipped.len())
                };
                Ok(JobDone {
                    message: format!(
                        "Imported \"{}\": {} tasks, {} notes, {} attachments{skipped}",
                        report.name, report.tasks, report.notes, report.attachments
                    ),
                    imported: Some(report.project_id),
                })
            });
        });
    }
}

/// One settings row: a title and hint on the left, the control on the right.
fn setting_row(
    theme: &GlassTheme,
    title: &str,
    hint: impl Into<SharedString>,
    control: impl IntoElement,
) -> gpui::Div {
    let theme = *theme;
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.space_6)
        .py(theme.space_4)
        .border_b_1()
        .border_color(theme.border_subtle)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    div()
                        .t_body()
                        .t_strong()
                        .text_color(theme.text_primary)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child(hint.into()),
                ),
        )
        .child(div().flex_none().child(control))
}

/// A titled group of setting rows.
fn setting_group(theme: &GlassTheme, title: &str, rows: Vec<gpui::Div>) -> gpui::Div {
    let theme = *theme;
    div()
        .flex()
        .flex_col()
        .child(div().pb(theme.space_1).child(section_label(&theme, title)))
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme.border_subtle)
                .children(rows),
        )
}

impl Render for WorkspaceTools {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let (command, theme_mode, reduce_transparency, reduce_motion, scrollback): (
            SharedString,
            _,
            _,
            _,
            _,
        ) = {
            let settings = self.settings.read(cx).get();
            (
                settings.ide_command.clone().into(),
                settings.theme_mode,
                settings.reduce_transparency,
                settings.reduce_motion,
                settings.terminal_scrollback,
            )
        };
        let busy = self.busy;
        let has_project = self.state.read(cx).selected_project.is_some();
        let db_path = self
            .state
            .read(cx)
            .workspace_paths()
            .database_file()
            .display()
            .to_string();

        let appearance = setting_group(
            &theme,
            "Appearance",
            vec![
                setting_row(
                    &theme,
                    "Theme",
                    "Dark is the default. System follows your operating system.",
                    segmented(&theme)
                        .child(
                            segment(&theme, "theme-system", "System", theme_mode == ThemeMode::System)
                                .on_click(cx.listener(|this, _, _, cx| this.set_theme(ThemeMode::System, cx))),
                        )
                        .child(
                            segment(&theme, "theme-dark", "Dark", theme_mode == ThemeMode::Dark)
                                .on_click(cx.listener(|this, _, _, cx| this.set_theme(ThemeMode::Dark, cx))),
                        )
                        .child(
                            segment(&theme, "theme-light", "Light", theme_mode == ThemeMode::Light)
                                .on_click(cx.listener(|this, _, _, cx| this.set_theme(ThemeMode::Light, cx))),
                        ),
                ),
                setting_row(
                    &theme,
                    "Reduce transparency",
                    "Solid surfaces instead of the translucent window. Easier to read over busy wallpapers.",
                    switch(&theme, "toggle-transparency", reduce_transparency)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_reduce_transparency(cx))),
                ),
                setting_row(
                    &theme,
                    "Reduce motion",
                    "Turn off animations and transitions. Controls still change state, just without the movement.",
                    switch(&theme, "toggle-motion", reduce_motion)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_reduce_motion(cx))),
                ),
            ],
        );

        let tools = setting_group(
            &theme,
            "Terminal and editor",
            vec![
                setting_row(
                    &theme,
                    "Terminal scrollback",
                    format!(
                        "Currently {scrollback} lines. Applies to terminals opened from now on."
                    ),
                    div().w(px(240.0)).child(self.scrollback_input.clone()),
                ),
                setting_row(
                    &theme,
                    "IDE command",
                    format!("Currently \"{command}\". Press Enter to save."),
                    div().w(px(240.0)).child(self.ide_input.clone()),
                ),
            ],
        );

        let data = setting_group(
            &theme,
            "Data",
            vec![
                setting_row(
                    &theme,
                    "Data location",
                    format!("{db_path}. Notes and attachments are in the folders beside it."),
                    button(
                        &theme,
                        "reveal-database",
                        ButtonKind::Secondary,
                        Some(Icon::FolderOpen),
                        "Show in folder",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.reveal_database(cx))),
                ),
                setting_row(
                    &theme,
                    "Back up workspace",
                    "Copies the database and every project's files to a dated folder.",
                    button(
                        &theme,
                        "backup-workspace",
                        ButtonKind::Secondary,
                        Some(Icon::Archive),
                        "Back up",
                    )
                    .when(busy, |b| b.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.backup(cx))),
                ),
                setting_row(
                    &theme,
                    "Export project",
                    if has_project {
                        "Writes the selected project, its notes and files to a folder you choose. Credentials are never exported."
                    } else {
                        "Select a project first."
                    },
                    button(
                        &theme,
                        "export-project",
                        ButtonKind::Secondary,
                        Some(Icon::Upload),
                        "Export…",
                    )
                    .when(busy || !has_project, |b| b.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                ),
                setting_row(
                    &theme,
                    "Import project",
                    "Adds an exported project as a new project. Nothing existing is changed.",
                    button(
                        &theme,
                        "import-project",
                        ButtonKind::Secondary,
                        Some(Icon::Download),
                        "Import…",
                    )
                    .when(busy, |b| b.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                ),
            ],
        );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .child(page_header(
                &theme,
                "Settings",
                Some("Appearance, terminal, editor and your data".into()),
                None,
                None,
            ))
            .child(
                div()
                    .id("settings-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .px(theme.space_6)
                            .py(theme.space_6)
                            .gap(theme.space_8)
                            // The three groups cascade in, 45 ms apart.
                            .child(motion::enter_after(
                                appearance,
                                "settings-group-0",
                                0,
                                motion::STANDARD,
                            ))
                            .child(motion::enter_after(
                                tools,
                                "settings-group-1",
                                1,
                                motion::STANDARD,
                            ))
                            .child(motion::enter_after(
                                data,
                                "settings-group-2",
                                2,
                                motion::STANDARD,
                            )),
                    ),
            )
    }
}
