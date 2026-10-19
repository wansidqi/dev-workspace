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
use crate::settings::{parse_scrollback, SettingsStore};
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;

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
        let ide_input =
            cx.new(|cx| TextInput::new(cx, "New IDE command, e.g. code --reuse-window", theme));
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
        let scrollback_input =
            cx.new(|cx| TextInput::new(cx, "Scrollback lines for new terminals, e.g. 5000", theme));
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

    fn cycle_theme(&mut self, cx: &mut Context<Self>) {
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.theme_mode = s.theme_mode.cycle())
        });
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

    fn open_in_ide(&mut self, cx: &mut Context<Self>) {
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

/// An outlined text button in the style of the page's other actions.
fn tool_button(
    theme: &GlassTheme,
    id: &'static str,
    label: impl Into<SharedString>,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let theme = *theme;
    div()
        .id(id)
        .flex_none()
        .px(theme.space_3)
        .py(theme.space_1)
        .rounded(theme.radius_md)
        .border_1()
        .text_sm()
        .when(enabled, |d| {
            d.border_color(theme.accent)
                .text_color(theme.accent)
                .cursor_pointer()
                .hover(move |s| s.bg(theme.surface_raised))
        })
        .when(!enabled, |d| {
            d.border_color(theme.border_subtle)
                .text_color(theme.text_secondary)
        })
        .child(label.into())
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
        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .p(theme.space_3)
            .rounded(theme.radius_md)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child("WORKSPACE TOOLS"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .child(
                        tool_button(&theme, "open-in-ide", "Open in IDE", true)
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_ide(cx))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(format!("IDE command: {command}")),
                    ),
            )
            .child(self.ide_input.clone())
            .child(
                div()
                    .mt(theme.space_2)
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child("APPEARANCE AND TERMINAL"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        tool_button(
                            &theme,
                            "cycle-theme",
                            format!("Theme: {}", theme_mode.label()),
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.cycle_theme(cx))),
                    )
                    .child(
                        tool_button(
                            &theme,
                            "toggle-transparency",
                            if reduce_transparency {
                                "Transparency: reduced"
                            } else {
                                "Transparency: normal"
                            },
                            true,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.toggle_reduce_transparency(cx)),
                        ),
                    )
                    .child(
                        tool_button(
                            &theme,
                            "toggle-motion",
                            if reduce_motion {
                                "Motion: reduced"
                            } else {
                                "Motion: normal"
                            },
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_reduce_motion(cx))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "Terminal scrollback: {scrollback} lines (applies to new terminals)"
                    )),
            )
            .child(self.scrollback_input.clone())
            .child(
                div()
                    .mt(theme.space_2)
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child("BACKUP AND TRANSFER"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.space_2)
                    .child(
                        tool_button(&theme, "backup-workspace", "Back up workspace", !busy)
                            .on_click(cx.listener(|this, _, _, cx| this.backup(cx))),
                    )
                    .child(
                        tool_button(&theme, "export-project", "Export project…", !busy)
                            .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                    )
                    .child(
                        tool_button(&theme, "import-project", "Import project…", !busy)
                            .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                    ),
            )
            .when_some(self.notice.clone(), |d, (message, is_error)| {
                d.child(
                    div()
                        .max_w(px(640.0))
                        .text_xs()
                        .text_color(if is_error {
                            theme.danger
                        } else {
                            theme.success
                        })
                        .child(message),
                )
            })
    }
}
