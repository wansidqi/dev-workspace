//! Monthly timesheet view: reads the month's commits from every active
//! project's linked repos, groups them into one task row per ticket number,
//! suggests hours, lets the user edit them, and copies the report as
//! tab-separated text for pasting into a spreadsheet.
//!
//! Logic lives in `devspace-core` (grouping, estimates, layout), storage in
//! `devspace-storage` (SQLite, migration 4) and git reading in `devspace-git`;
//! this file is only the view. Git is read on a background task, and every
//! edit is saved to the database straight away.

use std::collections::HashSet;
use std::path::PathBuf;

use devspace_core::{
    clipboard_text, date_from_unix, fmt_hours, fmt_range, group_commits, merge_rows, month_bounds,
    month_from_unix, month_label, shift_month, total_hours, Estimate, ProjectId, TimesheetCommit,
    TimesheetRow, TimesheetSettings,
};
use gpui::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, Context, Entity, SharedString, WeakEntity,
    Window,
};

use crate::state::{AppState, TimesheetProject};
use crate::text_input::TextInput;
use crate::theme::{GlassTheme, MONO_FONT};
use crate::{icon_button, icon_button_tinted};

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// Largest hour value accepted when typed; catches a stray extra digit.
const MAX_HOURS: f64 = 999.0;
/// Step of the − / + buttons next to a task's hours.
const HOURS_STEP: f64 = 0.5;

/// What the single inline editor is editing.
#[derive(Clone, PartialEq, Eq)]
enum Target {
    Title(ProjectId, String),
    Hours(ProjectId, String),
    Remarks,
}

struct Editor {
    target: Target,
    input: Entity<TextInput>,
}

/// The settings form: one input per field, in the order of `FIELD_LABELS`.
struct SettingsForm {
    inputs: Vec<Entity<TextInput>>,
    error: Option<String>,
}

const FIELD_LABELS: [&str; 10] = [
    "Name",
    "Role",
    "Sheet title",
    "Git authors (comma separated names or emails; empty = everyone)",
    "Blank rows to pad each project table to",
    "Estimate: base hours per task",
    "Estimate: hours per sqrt(changed lines)",
    "Estimate: hours per extra commit",
    "Estimate: hours per extra day",
    "Estimate: max hours per active day",
];

/// Settings as the strings the form shows, in `FIELD_LABELS` order.
fn settings_to_fields(s: &TimesheetSettings) -> [String; 10] {
    [
        s.name.clone(),
        s.role.clone(),
        s.title.clone(),
        s.authors.clone(),
        s.min_rows.to_string(),
        s.estimate.base.to_string(),
        s.estimate.line_factor.to_string(),
        s.estimate.per_extra_commit.to_string(),
        s.estimate.per_extra_day.to_string(),
        s.estimate.max_per_day.to_string(),
    ]
}

/// Validates the form's text into settings, naming the first bad field.
fn settings_from_fields(f: &[String]) -> Result<TimesheetSettings, String> {
    let number = |i: usize| -> Result<f64, String> {
        f[i].trim()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| format!("\"{}\" must be a number, 0 or more", FIELD_LABELS[i]))
    };
    let min_rows = f[4]
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n <= 100)
        .ok_or_else(|| format!("\"{}\" must be a whole number up to 100", FIELD_LABELS[4]))?;
    Ok(TimesheetSettings {
        name: f[0].trim().to_string(),
        role: f[1].trim().to_string(),
        title: f[2].trim().to_string(),
        authors: f[3].trim().to_string(),
        min_rows,
        estimate: Estimate {
            base: number(5)?,
            line_factor: number(6)?,
            per_extra_commit: number(7)?,
            per_extra_day: number(8)?,
            max_per_day: number(9)?,
        },
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Git result for one project: its commits and the repos that failed.
type ProjectCommits = (ProjectId, Vec<TimesheetCommit>, Vec<String>);

/// The "Timesheet" main-content view.
pub struct TimesheetPanel {
    state: Entity<AppState>,
    theme: GlassTheme,
    /// `YYYY-MM` being shown.
    month: String,
    settings: TimesheetSettings,
    projects: Vec<TimesheetProject>,
    tables: Vec<(ProjectId, Vec<TimesheetRow>)>,
    remarks: String,
    syncing: bool,
    /// Bumped when the month changes so a late git result is dropped.
    sync_epoch: u64,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    editor: Option<Editor>,
    settings_form: Option<SettingsForm>,
    /// Rows whose commit list is expanded.
    expanded: HashSet<(ProjectId, String)>,
    next_manual: u64,
    /// Called by the back button; the root hides the view.
    pub on_close: Option<CloseHandler>,
}

impl TimesheetPanel {
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            theme,
            month: month_from_unix(now_unix()),
            settings: TimesheetSettings::default(),
            projects: Vec::new(),
            tables: Vec::new(),
            remarks: String::new(),
            syncing: false,
            sync_epoch: 0,
            notice: None,
            editor: None,
            settings_form: None,
            expanded: HashSet::new(),
            next_manual: 0,
            on_close: None,
        }
    }

    fn set_notice(&mut self, message: impl Into<String>, is_error: bool) {
        self.notice = Some((message.into(), is_error));
    }

    /// Reloads settings, projects and the month from the database. A month
    /// that was never saved is filled from git right away.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        let (settings, projects, month) = {
            let state = self.state.read(cx);
            (
                state.timesheet_settings(),
                state.timesheet_projects(),
                state.load_timesheet_month(&self.month),
            )
        };
        self.settings = settings;
        self.projects = projects;
        match month {
            Ok(month) => {
                self.tables = month.tables;
                self.remarks = month.remarks;
                if !month.saved {
                    self.sync(cx);
                }
            }
            Err(message) => {
                self.tables.clear();
                self.remarks.clear();
                self.set_notice(format!("Could not load the month: {message}"), true);
            }
        }
        cx.notify();
    }

    fn change_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(month) = shift_month(&self.month, delta) else {
            return;
        };
        self.month = month;
        self.sync_epoch += 1;
        self.syncing = false;
        self.notice = None;
        self.refresh(cx);
    }

    /// Saves the whole month; called after every edit (the data is small).
    fn persist(&mut self, cx: &mut Context<Self>) {
        let result =
            self.state
                .read(cx)
                .save_timesheet_month(&self.month, &self.remarks, &self.tables);
        if let Err(message) = result {
            self.set_notice(format!("Could not save: {message}"), true);
        }
    }

    // ---- git sync ----

    /// Reads the month's commits for every project on a background task,
    /// then merges them into the tables, keeping typed hours, edited titles
    /// and manual rows.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.syncing {
            return;
        }
        let Some((from, to)) = month_bounds(&self.month) else {
            self.set_notice("Not a valid month", true);
            return;
        };
        self.settings = self.state.read(cx).timesheet_settings();
        self.projects = self.state.read(cx).timesheet_projects();
        self.syncing = true;
        self.notice = None;
        let epoch = self.sync_epoch;
        let projects = self.projects.clone();
        let authors = self.settings.author_list();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: Vec<ProjectCommits> = cx
                .background_executor()
                .spawn(async move {
                    projects
                        .into_iter()
                        .map(|project| {
                            let mut commits = Vec::new();
                            let mut failed = Vec::new();
                            for repo in &project.repos {
                                match devspace_git::timesheet_commits(
                                    &PathBuf::from(&repo.path),
                                    &repo.name,
                                    &from,
                                    &to,
                                    &authors,
                                ) {
                                    Ok(found) => commits.extend(found),
                                    Err(_) => failed.push(repo.name.clone()),
                                }
                            }
                            (project.id, commits, failed)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.sync_epoch == epoch {
                    this.finish_sync(result, cx);
                }
            });
        })
        .detach();
    }

    fn finish_sync(&mut self, result: Vec<ProjectCommits>, cx: &mut Context<Self>) {
        self.syncing = false;
        let mut failed: Vec<String> = Vec::new();
        let mut tables = Vec::with_capacity(result.len());
        for (project, commits, unreadable) in result {
            let previous = self
                .tables
                .iter()
                .find(|(id, _)| *id == project)
                .map(|(_, rows)| rows.as_slice())
                .unwrap_or_default();
            // A repo that could not be read must not wipe its saved rows:
            // keep this project's table as it was.
            if unreadable.is_empty() {
                let rows = merge_rows(group_commits(commits), previous, &self.settings.estimate);
                tables.push((project, rows));
            } else {
                tables.push((project, previous.to_vec()));
                failed.extend(unreadable);
            }
        }
        self.tables = tables;
        self.editor = None;
        self.persist(cx);
        if self.notice.is_none() {
            if failed.is_empty() {
                self.set_notice(
                    "Synced from git. Your hours and title edits were kept.",
                    false,
                );
            } else {
                self.set_notice(
                    format!(
                        "Could not read {}; those projects were left as they were.",
                        failed.join(", ")
                    ),
                    true,
                );
            }
        }
        cx.notify();
    }

    // ---- edits ----

    fn rows_mut(&mut self, project: ProjectId) -> Option<&mut Vec<TimesheetRow>> {
        self.tables
            .iter_mut()
            .find(|(id, _)| *id == project)
            .map(|(_, rows)| rows)
    }

    fn row_mut(&mut self, project: ProjectId, key: &str) -> Option<&mut TimesheetRow> {
        self.rows_mut(project)?.iter_mut().find(|r| r.key == key)
    }

    fn add_task(&mut self, project: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        let today = date_from_unix(now_unix());
        let date = if today.starts_with(&self.month) {
            today
        } else {
            format!("{}-01", self.month)
        };
        let mut key;
        loop {
            self.next_manual += 1;
            key = format!("manual:{}-{}", now_unix(), self.next_manual);
            if self.row_mut(project, &key).is_none() {
                break;
            }
        }
        let Some(rows) = self.rows_mut(project) else {
            return;
        };
        rows.push(TimesheetRow::manual(key.clone(), &date));
        self.persist(cx);
        self.begin_edit(Target::Title(project, key), String::new(), window, cx);
    }

    fn remove_row(&mut self, project: ProjectId, key: &str, cx: &mut Context<Self>) {
        if let Some(rows) = self.rows_mut(project) {
            rows.retain(|r| r.key != key);
        }
        self.expanded.remove(&(project, key.to_string()));
        self.editor = None;
        self.persist(cx);
        cx.notify();
    }

    fn step_hours(&mut self, project: ProjectId, key: &str, delta: f64, cx: &mut Context<Self>) {
        if let Some(row) = self.row_mut(project, key) {
            let current = row.hours.unwrap_or(0.0);
            row.hours = Some((current + delta).clamp(0.0, MAX_HOURS));
            // A hand-set value is never recalculated by a sync.
            row.auto = false;
        }
        self.persist(cx);
        cx.notify();
    }

    fn toggle_commits(&mut self, project: ProjectId, key: &str, cx: &mut Context<Self>) {
        let id = (project, key.to_string());
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        cx.notify();
    }

    fn begin_edit(
        &mut self,
        target: Target,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme;
        let placeholder = match target {
            Target::Title(..) => "Task description",
            Target::Hours(..) => "Hours (empty clears)",
            Target::Remarks => "e.g. no support work this month",
        };
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, placeholder, theme);
            input.content = initial.into();
            input
        });
        let weak: WeakEntity<TimesheetPanel> = cx.weak_entity();
        let confirm_panel = weak.clone();
        input.update(cx, |input, _| {
            // Enter and Escape run deferred: the input is mid-update (and is
            // cleared right after its handler), so the panel must not drop
            // it from inside the handler.
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                let panel = confirm_panel.clone();
                cx.defer(move |cx| {
                    panel
                        .update(cx, |panel, cx| panel.commit_edit(text, cx))
                        .ok();
                });
            }));
            input.on_cancel = Some(Box::new(move |_window, cx| {
                let panel = weak.clone();
                cx.defer(move |cx| {
                    panel
                        .update(cx, |panel, cx| {
                            panel.editor = None;
                            cx.notify();
                        })
                        .ok();
                });
            }));
        });
        let handle = input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
        self.editor = Some(Editor { target, input });
        cx.notify();
    }

    fn commit_edit(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        match editor.target {
            Target::Title(project, key) => {
                if let Some(row) = self.row_mut(project, &key) {
                    row.title = text.trim().to_string();
                }
            }
            Target::Hours(project, key) => {
                let text = text.trim();
                let hours = if text.is_empty() {
                    Ok(None)
                } else {
                    text.parse::<f64>()
                        .ok()
                        .filter(|h| h.is_finite() && (0.0..=MAX_HOURS).contains(h))
                        .map(Some)
                        .ok_or(())
                };
                match hours {
                    Ok(hours) => {
                        if let Some(row) = self.row_mut(project, &key) {
                            row.hours = hours;
                            row.auto = false;
                        }
                    }
                    Err(()) => self.set_notice(
                        format!("Hours must be a number from 0 to {MAX_HOURS}"),
                        true,
                    ),
                }
            }
            Target::Remarks => self.remarks = text.trim().to_string(),
        }
        self.persist(cx);
        cx.notify();
    }

    // ---- copy ----

    fn copy_all(&mut self, cx: &mut Context<Self>) {
        let tables: Vec<(String, Vec<TimesheetRow>)> = self
            .projects
            .iter()
            .map(|p| {
                let rows = self
                    .tables
                    .iter()
                    .find(|(id, _)| *id == p.id)
                    .map(|(_, rows)| rows.clone())
                    .unwrap_or_default();
                (p.name.clone(), rows)
            })
            .collect();
        let text = clipboard_text(&self.settings, &self.month, &self.remarks, &tables);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let label = month_label(&self.month);
        self.set_notice(
            format!(
                "Copied. In your timesheet add a tab named \"{label}\", click cell A1 and paste \
                 (Ctrl+V). Values and the SUM formulas come across; fonts and borders do not."
            ),
            false,
        );
        cx.notify();
    }

    // ---- settings form ----

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme;
        let values = settings_to_fields(&self.settings);
        let weak: WeakEntity<TimesheetPanel> = cx.weak_entity();
        let mut inputs = Vec::with_capacity(FIELD_LABELS.len());
        for (index, value) in values.into_iter().enumerate() {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(cx, "", theme);
                input.content = value.into();
                input
            });
            let save_panel = weak.clone();
            let cancel_panel = weak.clone();
            input.update(cx, |input, _| {
                // Enter saves, Escape closes; deferred for the same reason as
                // the inline editor. The pressed input is cleared right after
                // its handler, so its text is passed along.
                input.on_confirm = Some(Box::new(move |text, _window, cx| {
                    let text = text.to_string();
                    let panel = save_panel.clone();
                    cx.defer(move |cx| {
                        panel
                            .update(cx, |panel, cx| panel.save_settings(Some((index, text)), cx))
                            .ok();
                    });
                }));
                input.on_cancel = Some(Box::new(move |_window, cx| {
                    let panel = cancel_panel.clone();
                    cx.defer(move |cx| {
                        panel
                            .update(cx, |panel, cx| {
                                panel.settings_form = None;
                                cx.notify();
                            })
                            .ok();
                    });
                }));
            });
            inputs.push(input);
        }
        if let Some(first) = inputs.first() {
            let handle = first.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        }
        self.settings_form = Some(SettingsForm {
            inputs,
            error: None,
        });
        cx.notify();
    }

    fn save_settings(&mut self, pressed: Option<(usize, String)>, cx: &mut Context<Self>) {
        let Some(form) = &self.settings_form else {
            return;
        };
        let fields: Vec<String> = form
            .inputs
            .iter()
            .enumerate()
            .map(|(i, input)| match &pressed {
                Some((p, text)) if *p == i => text.clone(),
                _ => input.read(cx).content.to_string(),
            })
            .collect();
        let saved = settings_from_fields(&fields).and_then(|settings| {
            self.state.read(cx).save_timesheet_settings(&settings)?;
            Ok(settings)
        });
        match saved {
            Ok(settings) => {
                self.settings = settings;
                self.settings_form = None;
                self.set_notice(
                    "Settings saved. Sync from git to apply the new estimate.",
                    false,
                );
            }
            Err(message) => {
                if let Some(form) = &mut self.settings_form {
                    form.error = Some(message);
                }
            }
        }
        cx.notify();
    }

    // ---- rendering ----

    fn button(
        &self,
        label: &'static str,
        id: impl Into<gpui::ElementId>,
        primary: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .px(theme.space_3)
            .py(theme.space_1)
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(if primary {
                theme.accent
            } else {
                theme.border_light
            })
            .text_sm()
            .text_color(if primary {
                theme.accent
            } else {
                theme.text_secondary
            })
            .cursor_pointer()
            .hover(move |s| s.bg(theme.surface_raised).text_color(theme.text_primary))
            .child(label)
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let form = self.settings_form.as_ref()?;
        let fields = FIELD_LABELS.iter().zip(&form.inputs).map(|(label, input)| {
            div()
                .flex()
                .flex_col()
                .gap(theme.space_1)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(*label),
                )
                .child(input.clone())
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .p(theme.space_3)
                .rounded(theme.radius_md)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border_light)
                .child(
                    div()
                        .text_color(theme.text_primary)
                        .child("Timesheet settings"),
                )
                .children(fields)
                .when_some(form.error.clone(), |el, message| {
                    el.child(div().text_xs().text_color(theme.danger).child(message))
                })
                .child(
                    div()
                        .flex()
                        .gap(theme.space_2)
                        .child(
                            self.button("Save", "ts-settings-save", true).on_click(
                                cx.listener(|this, _, _, cx| this.save_settings(None, cx)),
                            ),
                        )
                        .child(self.button("Cancel", "ts-settings-cancel", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.settings_form = None;
                                cx.notify();
                            }),
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child("Enter saves, Esc cancels"),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The text of a cell: the inline input while it is being edited, else
    /// a clickable label.
    fn editable(
        &self,
        target: Target,
        id: impl Into<gpui::ElementId>,
        shown: SharedString,
        initial: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        if let Some(editor) = self.editor.as_ref().filter(|e| e.target == target) {
            return editor.input.clone().into_any_element();
        }
        let empty = shown.is_empty();
        div()
            .id(id)
            .px(theme.space_2)
            .py(theme.space_1)
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .text_color(if empty {
                theme.text_secondary
            } else {
                theme.text_primary
            })
            .hover(move |s| s.bg(theme.surface_raised).border_color(theme.border_subtle))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.begin_edit(target.clone(), initial.clone(), window, cx);
            }))
            .child(if empty {
                SharedString::from("(click to edit)")
            } else {
                shown
            })
            .into_any_element()
    }

    fn render_row(
        &self,
        project: ProjectId,
        index: usize,
        row: &TimesheetRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let uid = (project.0 as u64) * 100_000 + index as u64;
        let key = row.key.clone();
        let open = self.expanded.contains(&(project, key.clone()));
        let stats = format!(
            "{} commit{} · {} day{} · {} lines",
            row.commits.len(),
            if row.commits.len() == 1 { "" } else { "s" },
            row.day_count(),
            if row.day_count() == 1 { "" } else { "s" },
            row.line_count(),
        );

        let title = self.editable(
            Target::Title(project, key.clone()),
            ("ts-title", uid),
            row.title.clone().into(),
            row.title.clone(),
            cx,
        );
        let hours = self.editable(
            Target::Hours(project, key.clone()),
            ("ts-hours", uid),
            row.hours.map(fmt_hours).unwrap_or_default().into(),
            row.hours.map(fmt_hours).unwrap_or_default(),
            cx,
        );

        let (step_key_a, step_key_b, remove_key, toggle_key) =
            (key.clone(), key.clone(), key.clone(), key.clone());
        div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .py(theme.space_2)
            .border_t_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .child(
                        div()
                            .w(px(24.0))
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child((index + 1).to_string()),
                    )
                    .child(div().flex_1().min_w(px(0.0)).child(title))
                    .child(
                        div()
                            .w(px(150.0))
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(fmt_range(&row.date_from, &row.date_to)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_1)
                            .w(px(150.0))
                            .child(icon_button(&theme, "−", ("ts-minus", uid)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.step_hours(project, &step_key_a, -HOURS_STEP, cx)
                                }),
                            ))
                            .child(div().w(px(56.0)).child(hours))
                            .child(icon_button(&theme, "+", ("ts-plus", uid)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.step_hours(project, &step_key_b, HOURS_STEP, cx)
                                }),
                            ))
                            .child(div().text_xs().text_color(theme.text_secondary).child(
                                if row.auto && row.hours.is_some() {
                                    "h est."
                                } else {
                                    "h"
                                },
                            )),
                    )
                    .child(
                        icon_button_tinted(&theme, "✕", ("ts-remove", uid), theme.danger).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.remove_row(project, &remove_key, cx)
                            }),
                        ),
                    ),
            )
            .when(!row.commits.is_empty(), |el| {
                el.child(
                    div()
                        .id(("ts-commits", uid))
                        .ml(px(24.0))
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(theme.text_primary))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_commits(project, &toggle_key, cx)
                        }))
                        .child(format!("{} {stats}", if open { "▾" } else { "▸" })),
                )
            })
            .when(open, |el| {
                el.child(
                    div()
                        .ml(px(24.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .p(theme.space_2)
                        .rounded(theme.radius_sm)
                        .bg(theme.surface_solid)
                        .children(row.commits.iter().map(|c| {
                            div()
                                .flex()
                                .gap(theme.space_2)
                                .text_xs()
                                .child(
                                    div()
                                        .font_family(MONO_FONT)
                                        .text_color(theme.accent)
                                        .child(c.hash.chars().take(7).collect::<String>()),
                                )
                                .child(div().text_color(theme.text_secondary).child(c.date.clone()))
                                .child(div().text_color(theme.text_secondary).child(c.repo.clone()))
                                .child(
                                    div()
                                        .text_color(theme.text_primary)
                                        .child(c.subject.clone()),
                                )
                        })),
                )
            })
            .into_any_element()
    }

    fn render_project(&self, project: &TimesheetProject, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let rows: &[TimesheetRow] = self
            .tables
            .iter()
            .find(|(id, _)| *id == project.id)
            .map(|(_, rows)| rows.as_slice())
            .unwrap_or_default();
        let id = project.id;
        let rendered: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| self.render_row(id, i, row, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .p(theme.space_3)
            .rounded(theme.radius_md)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_light)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb(theme.space_2)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                div()
                                    .text_color(theme.text_primary)
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.accent)
                                    .child(format!("{} h", fmt_hours(total_hours(rows)))),
                            )
                            .when(project.repos.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.warning)
                                        .child("no repos linked"),
                                )
                            }),
                    )
                    .child(
                        self.button("+ Add task", ("ts-add", id.0 as u64), false)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_task(id, window, cx)
                            })),
                    ),
            )
            .when(rows.is_empty(), |el| {
                el.child(
                    div()
                        .py(theme.space_3)
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child("No commits found this month."),
                )
            })
            .children(rendered)
            .into_any_element()
    }
}

impl Render for TimesheetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let tasks: usize = self.tables.iter().map(|(_, rows)| rows.len()).sum();
        let commits: usize = self
            .tables
            .iter()
            .flat_map(|(_, rows)| rows)
            .map(|r| r.commits.len())
            .sum();
        let hours: f64 = self.tables.iter().map(|(_, rows)| total_hours(rows)).sum();
        let projects = self.projects.clone();
        let sections: Vec<AnyElement> = projects
            .iter()
            .map(|p| self.render_project(p, cx))
            .collect();

        let remarks = self.editable(
            Target::Remarks,
            "ts-remarks",
            self.remarks.clone().into(),
            self.remarks.clone(),
            cx,
        );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .bg(theme.surface_solid)
            .p(theme.space_4)
            .gap(theme.space_3)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(icon_button(&theme, "←", "ts-back").on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let Some(close) = &this.on_close {
                                        close(window, cx);
                                    }
                                },
                            )))
                            .child(
                                div()
                                    .text_xl()
                                    .text_color(theme.text_primary)
                                    .child("Timesheet"),
                            )
                            .child(
                                icon_button(&theme, "‹", "ts-prev").on_click(
                                    cx.listener(|this, _, _, cx| this.change_month(-1, cx)),
                                ),
                            )
                            .child(
                                div()
                                    .w(px(150.0))
                                    .text_center()
                                    .text_color(theme.text_primary)
                                    .child(month_label(&self.month)),
                            )
                            .child(
                                icon_button(&theme, "›", "ts-next").on_click(
                                    cx.listener(|this, _, _, cx| this.change_month(1, cx)),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme.space_2)
                            .child(
                                self.button(
                                    if self.syncing {
                                        "Syncing…"
                                    } else {
                                        "Sync from git"
                                    },
                                    "ts-sync",
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
                            )
                            .child(self.button("Settings", "ts-settings", false).on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ))
                            .child(
                                self.button("Copy all", "ts-copy", true)
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_all(cx))),
                            ),
                    ),
            )
            .when_some(self.notice.clone(), |el, (message, is_error)| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(if is_error {
                            theme.danger
                        } else {
                            theme.text_secondary
                        })
                        .child(message),
                )
            })
            .when_some(self.render_settings(cx), |el, form| el.child(form))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "{} h · {tasks} task{} · {commits} commit{}",
                        fmt_hours(hours),
                        if tasks == 1 { "" } else { "s" },
                        if commits == 1 { "" } else { "s" },
                    )),
            )
            .child(
                div()
                    .id("ts-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_3)
                    .overflow_y_scroll()
                    .when(projects.is_empty(), |el| {
                        el.child(
                            div()
                                .text_color(theme.text_secondary)
                                .child("No projects yet. Create one and link its git repos."),
                        )
                    })
                    .children(sections)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space_1)
                            .p(theme.space_3)
                            .rounded(theme.radius_md)
                            .bg(theme.surface_raised)
                            .border_1()
                            .border_color(theme.border_light)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child("REMARKS"),
                            )
                            .child(remarks),
                    ),
            )
    }
}

impl crate::Root {
    /// Shows or hides the timesheet in place of the main content. Opening it
    /// closes the other main-content views and reloads the month.
    pub(crate) fn toggle_timesheet_view(&mut self, cx: &mut Context<Self>) {
        if self.timesheet_open {
            self.timesheet_open = false;
        } else {
            self.open_history = None;
            self.open_task = None;
            self.open_note = None;
            self.run_open = false;
            self.timesheet_open = true;
            self.timesheet_panel
                .update(cx, |panel, cx| panel.refresh(cx));
        }
        cx.notify();
    }

    /// Sidebar row that opens the timesheet.
    pub(crate) fn timesheet_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let open = self.timesheet_open;
        div()
            .id("open-timesheet")
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
            .hover(move |s| s.bg(theme.surface_raised).border_color(theme.border_subtle))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_timesheet_view(cx)))
            .child(div().size(px(6.0)).rounded_full().bg(if open {
                theme.accent
            } else {
                gpui::transparent_black()
            }))
            .child(div().text_color(theme.text_secondary).child("Timesheet"))
    }
}
