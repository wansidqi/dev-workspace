//! Per-project run commands with a live log view (CLAUDE.md feature 7).
//!
//! `RunPanel` is owned by the root view for the whole app lifetime (like the
//! terminal panel) so commands keep running while the user navigates away or
//! switches project. The view only shows the selected project's commands;
//! running processes of other projects stay alive in `runs` until they are
//! stopped, their project is archived, or the app quits.
//!
//! All process work goes through `devspace-runner`; output is polled from a
//! background timer and never blocks the UI thread. The log is rendered with
//! a `uniform_list`, so only visible lines are laid out.

use crate::theme::TypeScale;
use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use devspace_core::{ProjectId, RunCommandId};
use devspace_runner::{RunState, RunningCommand};
use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, App, ClipboardItem, Context, Entity, Hsla,
    PathPromptOptions, SharedString, UniformListScrollHandle, WeakEntity, Window,
};

use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;
use crate::ui::{self, icon, section_label, ButtonKind, Icon};
use crate::{icon_button, icon_button_tinted, Root};

/// How often running commands are polled for new output. Same cadence as
/// the terminal view; only triggers a repaint when something changed.
const POLL_INTERVAL: Duration = Duration::from_millis(120);
/// Fixed log line height (uniform_list needs uniform rows).
const LOG_LINE_HEIGHT: f32 = 18.0;
/// Monospace UI font for the log, same family the commit browser uses for
/// hashes.
use crate::theme::TERMINAL_FONT;

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// A started command and what the UI last saw of its state (to repaint only
/// on change).
struct Run {
    project: ProjectId,
    cmd: RunningCommand,
    last_state: RunState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormMode {
    New,
    Edit(RunCommandId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Command,
    Cwd,
}

/// The add/edit form. Inputs are rebuilt (not reused) on every open so
/// stale text and cursor state never leak between edits.
struct Form {
    mode: FormMode,
    name: Entity<TextInput>,
    command: Entity<TextInput>,
    cwd: Entity<TextInput>,
    error: Option<String>,
}

/// The "Run commands" main-content view.
pub struct RunPanel {
    state: Entity<AppState>,
    theme: GlassTheme,
    runs: HashMap<RunCommandId, Run>,
    selected: Option<RunCommandId>,
    form: Option<Form>,
    confirm_delete: Option<RunCommandId>,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    scroll: UniformListScrollHandle,
    /// Called by the back button; the root hides the view.
    pub on_close: Option<CloseHandler>,
}

/// Status chip text and colour for a run state.
fn state_chip(theme: &GlassTheme, state: Option<RunState>) -> Option<(String, Hsla)> {
    Some(match state? {
        RunState::Running => ("Running".to_string(), theme.accent),
        RunState::Exited(Some(0)) => ("Exited 0".to_string(), theme.success),
        RunState::Exited(Some(code)) => (format!("Failed {code}"), theme.danger),
        RunState::Exited(None) => ("Killed".to_string(), theme.danger),
        RunState::Stopped => ("Stopped".to_string(), theme.warning),
    })
}

impl RunPanel {
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // Kill every child when the app quits so none are orphaned.
        cx.on_app_quit(|this, _cx| {
            this.stop_all();
            async {}
        })
        .detach();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            if this.update(cx, |panel, cx| panel.pump(cx)).is_err() {
                break;
            }
        })
        .detach();
        Self {
            state,
            theme,
            runs: HashMap::new(),
            selected: None,
            form: None,
            confirm_delete: None,
            notice: None,
            scroll: UniformListScrollHandle::new(),
            on_close: None,
        }
    }

    /// How many commands are currently running, for the sidebar badge.
    pub fn running_count(&self) -> usize {
        self.runs
            .values()
            .filter(|r| r.last_state == RunState::Running)
            .count()
    }

    /// Kills all commands (app quit).
    fn stop_all(&mut self) {
        for run in self.runs.values_mut() {
            run.cmd.stop();
        }
    }

    /// Drains output, notices exits, kills runs of archived projects, and
    /// repaints only if something changed.
    fn pump(&mut self, cx: &mut Context<Self>) {
        let live: Vec<ProjectId> = self.state.read(cx).projects.iter().map(|p| p.id).collect();
        let before = self.runs.len();
        // Dropping a `Run` kills its process tree.
        self.runs.retain(|_, run| live.contains(&run.project));
        let mut changed = self.runs.len() != before;

        let selected = self.selected_id(cx);
        for (id, run) in self.runs.iter_mut() {
            // Follow the tail only if the user has not scrolled up.
            let follow = self.scroll.is_scrolled_to_end().unwrap_or(true);
            let added = run.cmd.drain_output();
            let state = run.cmd.state();
            if added > 0 || state != run.last_state {
                changed = true;
                run.last_state = state;
                if added > 0 && follow && Some(*id) == selected {
                    self.scroll.scroll_to_bottom();
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// The command whose log is shown: the user's pick if it still exists,
    /// else the first of the project.
    fn selected_id(&self, cx: &App) -> Option<RunCommandId> {
        let commands = &self.state.read(cx).run_commands;
        self.selected
            .filter(|id| commands.iter().any(|c| c.id == *id))
            .or_else(|| commands.first().map(|c| c.id))
    }

    fn select(&mut self, id: RunCommandId, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn set_notice(&mut self, message: impl Into<String>, is_error: bool) {
        self.notice = Some((message.into(), is_error));
    }

    /// Starts (or restarts) a command. The cwd is validated by the runner so
    /// a missing folder yields a visible error instead of a silent failure.
    fn start(&mut self, id: RunCommandId, cx: &mut Context<Self>) {
        let (cmd, cwd, project) = {
            let state = self.state.read(cx);
            let Some(cmd) = state.run_commands.iter().find(|c| c.id == id).cloned() else {
                return;
            };
            let cwd = state.run_command_cwd(&cmd);
            (cmd.clone(), cwd, cmd.project_id)
        };
        if self
            .runs
            .get_mut(&id)
            .is_some_and(|run| run.cmd.is_running())
        {
            return;
        }
        match RunningCommand::spawn(&cmd.command, &cwd) {
            Ok(running) => {
                self.runs.insert(
                    id,
                    Run {
                        project,
                        cmd: running,
                        last_state: RunState::Running,
                    },
                );
                self.notice = None;
                self.select(id, cx);
            }
            Err(err) => {
                self.set_notice(format!("Could not start \"{}\": {err}", cmd.name), true);
                cx.notify();
            }
        }
    }

    fn stop(&mut self, id: RunCommandId, cx: &mut Context<Self>) {
        if let Some(run) = self.runs.get_mut(&id) {
            run.cmd.stop();
            run.last_state = run.cmd.state();
        }
        cx.notify();
    }

    fn delete(&mut self, id: RunCommandId, cx: &mut Context<Self>) {
        // Dropping the run kills it.
        self.runs.remove(&id);
        self.confirm_delete = None;
        self.state.update(cx, |state, cx| {
            state.delete_run_command(id);
            cx.notify();
        });
        cx.notify();
    }

    fn clear_log(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.selected_id(cx).and_then(|id| self.runs.get_mut(&id)) {
            run.cmd.clear_log();
        }
        cx.notify();
    }

    fn copy_log(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.selected_id(cx).and_then(|id| self.runs.get(&id)) else {
            return;
        };
        let text = run.cmd.log().to_text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.set_notice("Log copied to the clipboard", false);
        cx.notify();
    }

    // ---- add / edit form ----

    fn open_form(
        &mut self,
        mode: FormMode,
        values: (String, String, String),
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme;
        let make = |placeholder: &'static str, value: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(cx, placeholder, theme);
                input.content = value.into();
                input
            })
        };
        let name = make("Name (e.g. Dev server)", values.0, cx);
        let command = make("Command (e.g. npm run dev)", values.1, cx);
        let cwd = make(
            "Working directory (optional, default: first repo or home)",
            values.2,
            cx,
        );
        for (field, input) in [
            (Field::Name, &name),
            (Field::Command, &command),
            (Field::Cwd, &cwd),
        ] {
            self.wire(input, field, cx);
        }
        self.form = Some(Form {
            mode,
            name,
            command,
            cwd,
            error,
        });
        cx.notify();
    }

    /// Enter saves, Escape cancels. Both run deferred: the pressed input is
    /// mid-update (and is cleared right after its handler), so the panel must
    /// not touch or drop it from inside the handler.
    fn wire(&self, input: &Entity<TextInput>, field: Field, cx: &mut Context<Self>) {
        let weak: WeakEntity<RunPanel> = cx.weak_entity();
        let save_panel = weak.clone();
        input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                let panel = save_panel.clone();
                cx.defer(move |cx| {
                    panel
                        .update(cx, |panel, cx| panel.save_form(Some((field, text)), cx))
                        .ok();
                });
            }));
            input.on_cancel = Some(Box::new(move |_window, cx| {
                let panel = weak.clone();
                cx.defer(move |cx| {
                    panel
                        .update(cx, |panel, cx| {
                            panel.form = None;
                            cx.notify();
                        })
                        .ok();
                });
            }));
        });
    }

    /// Puts `path` in the form's working directory box (empty = default).
    fn set_cwd(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(form) = &self.form {
            form.cwd.update(cx, |input, cx| {
                input.set_text_selected(path);
                cx.notify();
            });
        }
        // Re-render so the chip matching the new path lights up.
        cx.notify();
    }

    /// Opens the folder picker and fills the working directory with the choice.
    fn browse_cwd(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose working directory".into()),
        });
        cx.spawn(async move |this, cx| {
            // Err = picker unavailable, Ok(None) = cancelled.
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(chosen) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.set_cwd(chosen.to_string_lossy().to_string(), cx);
            });
        })
        .detach();
    }

    /// Saves the form. `pressed` carries the text of the input Enter was hit
    /// in, since that input has already been cleared by then.
    fn save_form(&mut self, pressed: Option<(Field, String)>, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        let value = |field: Field, input: &Entity<TextInput>| -> String {
            match &pressed {
                Some((f, text)) if *f == field => text.clone(),
                _ => input.read(cx).content.to_string(),
            }
        };
        let name = value(Field::Name, &form.name);
        let command = value(Field::Command, &form.command);
        let cwd = value(Field::Cwd, &form.cwd);
        let mode = form.mode;

        let result = self.state.update(cx, |state, cx| {
            let result = match mode {
                FormMode::New => state.create_run_command(&name, &command, &cwd).map(Some),
                FormMode::Edit(id) => state
                    .update_run_command(id, &name, &command, &cwd)
                    .map(|()| None),
            };
            cx.notify();
            result
        });
        match result {
            Ok(created) => {
                self.form = None;
                if let Some(id) = created {
                    self.select(id, cx);
                }
                self.set_notice("Saved", false);
                cx.notify();
            }
            // Rebuild the form so the pressed input keeps its text.
            Err(message) => self.open_form(mode, (name, command, cwd), Some(message), cx),
        }
    }

    // ---- rendering helpers ----

    fn button(
        &self,
        label: &'static str,
        id: impl Into<gpui::ElementId>,
        primary: bool,
    ) -> gpui::Stateful<gpui::Div> {
        ui::button(
            &self.theme,
            id,
            if primary {
                ButtonKind::Primary
            } else {
                ButtonKind::Secondary
            },
            None,
            label,
        )
    }

    /// Click-to-fill choices for the working directory: the project default,
    /// each linked repo, and a folder picker.
    fn render_cwd_choices(&self, form: &Form, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let current = form.cwd.read(cx).content.trim().to_string();
        let repos = self.state.read(cx).repos.clone();
        let mut chips: Vec<AnyElement> = Vec::new();
        chips.push(
            ui::filter_chip(&theme, "run-cwd-default", "Project default", current.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.set_cwd(String::new(), cx)))
                .into_any_element(),
        );
        for repo in repos {
            let path = repo.path.clone();
            chips.push(
                ui::filter_chip(
                    &theme,
                    ("run-cwd-repo", repo.id.0 as u64),
                    repo.name.clone(),
                    current == repo.path,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_cwd(path.clone(), cx)))
                .into_any_element(),
            );
        }
        chips.push(
            ui::button(&theme, "run-cwd-browse", ButtonKind::Ghost, Some(Icon::FolderOpen), "Browse")
                .on_click(cx.listener(|this, _, _, cx| this.browse_cwd(cx)))
                .into_any_element(),
        );
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.space_2)
            .children(chips)
    }

    fn render_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let form = self.form.as_ref()?;
        let title = match form.mode {
            FormMode::New => "New run command",
            FormMode::Edit(_) => "Edit run command",
        };
        let field = |label: &'static str, input: &Entity<TextInput>| {
            div()
                .flex()
                .flex_col()
                .gap(theme.space_1)
                .child(section_label(&theme, label))
                .child(input.clone())
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(theme.space_3)
                .p(theme.space_4)
                .rounded(theme.radius_lg)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border_light)
                .child(
                    div()
                        .t_heading()
                        .text_color(theme.text_primary)
                        .child(title),
                )
                .child(field("Name", &form.name))
                .child(field("Command", &form.command))
                .child(field("Working directory", &form.cwd))
                .child(self.render_cwd_choices(form, cx))
                .when_some(form.error.clone(), |el, message| {
                    el.child(ui::notice(&theme, message, true))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .child(
                            self.button("Save", "run-form-save", true)
                                .on_click(cx.listener(|this, _, _, cx| this.save_form(None, cx))),
                        )
                        .child(
                            ui::button(
                                &theme,
                                "run-form-cancel",
                                ButtonKind::Ghost,
                                None,
                                "Cancel",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            div()
                                .t_small()
                                .text_color(theme.text_muted)
                                .child("Enter saves, Esc cancels"),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_command_row(
        &mut self,
        cmd: &devspace_core::RunCommand,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let id = cmd.id;
        let key = id.0 as u64;
        let (state, running) = match self.runs.get_mut(&id) {
            Some(run) => {
                let state = run.cmd.state();
                (Some(state), state == RunState::Running)
            }
            None => (None, false),
        };
        let state_chip = state_chip(&theme, state);
        let cwd_label = self.state.read(cx).run_command_cwd(cmd);
        let edit_values = (
            cmd.name.clone(),
            cmd.command.clone(),
            cmd.cwd.clone().unwrap_or_default(),
        );
        let confirming = self.confirm_delete == Some(id);

        div()
            .id(("run-cmd", key))
            .flex()
            .items_center()
            .gap(theme.space_3)
            .px(theme.space_3)
            .py(theme.space_3)
            .rounded(theme.radius_md)
            .border_1()
            .border_color(if selected {
                theme.accent.opacity(0.3)
            } else {
                gpui::transparent_black()
            })
            .when(selected, |d| d.bg(theme.accent.opacity(0.08)))
            .when(!selected, |d| d.hover(move |s| s.bg(theme.surface_hover)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(icon(
                if running {
                    Icon::CircleDot
                } else {
                    Icon::Terminal
                },
                16.0,
                if running {
                    theme.accent
                } else {
                    theme.text_muted
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                div()
                                    .t_body()
                                    .t_strong()
                                    .text_color(theme.text_primary)
                                    .child(cmd.name.clone()),
                            )
                            .when_some(state_chip, |el, (label, color)| {
                                el.child(ui::chip(&theme, label, color))
                            }),
                    )
                    .child(
                        div()
                            .t_small()
                            .font_family(TERMINAL_FONT)
                            .text_color(theme.text_secondary)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(cmd.command.clone()),
                    )
                    .child(
                        div()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(format!("in {}", cwd_label.display())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space_1)
                    .child(if running {
                        ui::button(
                            &theme,
                            ("run-stop", key),
                            ButtonKind::Secondary,
                            Some(Icon::Square),
                            "Stop",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stop(id, cx);
                            cx.stop_propagation();
                        }))
                    } else {
                        ui::button(
                            &theme,
                            ("run-start", key),
                            ButtonKind::Primary,
                            Some(Icon::Play),
                            "Start",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start(id, cx);
                            cx.stop_propagation();
                        }))
                    })
                    .child(
                        icon_button(&theme, "✎", ("run-edit", key)).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_form(FormMode::Edit(id), edit_values.clone(), None, cx);
                                this.focus_form_name(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(if confirming {
                        ui::button(
                            &theme,
                            ("run-delete", key),
                            ButtonKind::Danger,
                            Some(Icon::Trash),
                            "Delete?",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.delete(id, cx);
                            cx.stop_propagation();
                        }))
                        .into_any_element()
                    } else {
                        icon_button_tinted(&theme, "✕", ("run-delete", key), theme.danger)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm_delete = Some(id);
                                cx.notify();
                                cx.stop_propagation();
                            }))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn focus_form_name(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = &self.form {
            let handle = form.name.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        }
    }

    fn render_log(&mut self, selected: Option<RunCommandId>, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let (title, state, count, dropped) = {
            let name = selected.and_then(|id| {
                self.state
                    .read(cx)
                    .run_commands
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.name.clone())
            });
            let run = selected.and_then(|id| self.runs.get_mut(&id));
            match (name, run) {
                (Some(name), Some(run)) => {
                    let state = run.cmd.state();
                    (
                        name,
                        Some(state),
                        run.cmd.log().len(),
                        run.cmd.log().dropped(),
                    )
                }
                (Some(name), None) => (name, None, 0, 0),
                _ => (String::new(), None, 0, 0),
            }
        };
        let status = state_chip(&theme, state);
        let empty_message = if selected.is_none() {
            "Select a command to see its log."
        } else if state.is_none() {
            "Not started yet. Press Start to run it."
        } else {
            "No output yet."
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .rounded(theme.radius_md)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(theme.border_light)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(40.0))
                    .px(theme.space_3)
                    .border_b_1()
                    .border_color(theme.border_subtle)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(section_label(
                                &theme,
                                &if title.is_empty() {
                                    "Log".to_string()
                                } else {
                                    format!("Log · {title}")
                                },
                            ))
                            .when_some(status, |el, (label, color)| {
                                el.child(ui::chip(&theme, label, color))
                            })
                            .when(dropped > 0, |el| {
                                el.child(
                                    div()
                                        .t_small()
                                        .text_color(theme.text_muted)
                                        .child(format!("{dropped} older lines dropped")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme.space_1)
                            .child(
                                ui::button(
                                    &theme,
                                    "run-copy-log",
                                    ButtonKind::Ghost,
                                    Some(Icon::Copy),
                                    "Copy",
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.copy_log(cx))),
                            )
                            .child(
                                ui::button(
                                    &theme,
                                    "run-clear-log",
                                    ButtonKind::Ghost,
                                    Some(Icon::Trash),
                                    "Clear",
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.clear_log(cx))),
                            ),
                    ),
            )
            .child(if count == 0 {
                div()
                    .p(theme.space_4)
                    .t_body()
                    .text_color(theme.text_muted)
                    .child(empty_message)
                    .into_any_element()
            } else {
                uniform_list(
                    "run-log",
                    count,
                    cx.processor(move |this, range: Range<usize>, _window, _cx| {
                        let theme = this.theme;
                        let Some(run) = selected.and_then(|id| this.runs.get(&id)) else {
                            return Vec::new();
                        };
                        let log = run.cmd.log();
                        range
                            .filter_map(|ix| {
                                let line = log.get(ix)?;
                                Some(
                                    div()
                                        .id(("run-log-line", ix))
                                        .h(px(LOG_LINE_HEIGHT))
                                        .px(theme.space_3)
                                        .t_small()
                                        .font_family(TERMINAL_FONT)
                                        .text_color(theme.text_primary)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(SharedString::from(line.to_string())),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .w_full()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for RunPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let commands = self.state.read(cx).run_commands.clone();
        let selected = self.selected_id(cx);
        let has_project = self.state.read(cx).selected_project.is_some();
        let project_name = {
            let state = self.state.read(cx);
            state
                .selected_project
                .and_then(|id| state.projects.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
        };

        let mut list: Vec<AnyElement> = Vec::new();
        for cmd in &commands {
            list.push(self.render_command_row(cmd, selected == Some(cmd.id), cx));
        }

        let add = (has_project && self.form.is_none()).then(|| {
            ui::button(
                &theme,
                "run-add",
                ButtonKind::Primary,
                Some(Icon::Plus),
                "Add command",
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_form(
                    FormMode::New,
                    (String::new(), String::new(), String::new()),
                    None,
                    cx,
                );
                this.focus_form_name(window, cx);
            }))
            .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .child(ui::page_header(
                &theme,
                "Run commands",
                project_name.map(|n| format!("Saved commands for {n}").into()),
                Some(
                    ui::back_button(&theme, "run-back")
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(close) = &this.on_close {
                                close(window, cx);
                            }
                        }))
                        .into_any_element(),
                ),
                add,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_4)
                    .px(theme.space_6)
                    .py(theme.space_4)
                    .when_some(self.notice.clone(), |el, (message, is_error)| {
                        el.child(ui::notice(&theme, message, is_error))
                    })
                    .when_some(self.render_form(cx), |el, form| el.child(form))
                    .when(commands.is_empty() && self.form.is_none(), |el| {
                        el.child(ui::empty_state(
                            &theme,
                            Icon::Play,
                            if has_project {
                                "No run commands yet"
                            } else {
                                "No project selected"
                            },
                            if has_project {
                                "Save the commands you run often (dev server, tests, builds) and watch their output here."
                            } else {
                                "Select a project to manage its run commands."
                            },
                            None,
                        ))
                    })
                    .when(!list.is_empty(), |el| {
                        el.child(
                            div()
                                .id("run-command-list")
                                .flex()
                                .flex_col()
                                .flex_none()
                                .max_h(px(280.0))
                                .gap(px(2.0))
                                .overflow_y_scroll()
                                .children(list),
                        )
                    })
                    .child(self.render_log(selected, cx)),
            )
    }
}

impl Root {
    /// Shows or hides the run-commands view in place of the main content.
    /// Opening it closes the other main-content views, like the commit
    /// browser does.
    pub(crate) fn toggle_run_view(&mut self, cx: &mut Context<Self>) {
        if self.run_open {
            self.run_open = false;
        } else {
            self.open_history = None;
            self.open_task = None;
            self.open_note = None;
            self.settings_open = false;
            self.timesheet_open = false;
            self.run_open = true;
        }
        cx.notify();
    }

    /// Sidebar row that opens the run-commands view, with a badge counting
    /// running commands.
    pub(crate) fn run_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let running = self.run_panel.read(cx).running_count();
        let badge = (running > 0).then(|| {
            div()
                .px(theme.space_1)
                .rounded(theme.radius_sm)
                .border_1()
                .border_color(theme.accent)
                .t_small()
                .text_color(theme.accent)
                .child(format!("▶ {running}"))
                .into_any_element()
        });
        self.nav_row(
            crate::NAV_RUN,
            self.run_open,
            badge,
            |this, cx| this.toggle_run_view(cx),
            cx,
        )
    }
}
