//! Monthly timesheet view: reads the month's commits from every active
//! project's linked repos, groups them into one task row per ticket number,
//! suggests hours, lets the user edit them, and copies the report as
//! tab-separated text for pasting into a spreadsheet.
//!
//! Logic lives in `devspace-core` (grouping, estimates, layout), storage in
//! `devspace-storage` (SQLite, migration 4) and git reading in `devspace-git`;
//! this file is only the view. Git is read on a background task, and every
//! edit is saved to the database straight away.

use crate::theme::TypeScale;
use std::collections::HashSet;
use std::path::PathBuf;

use devspace_core::{
    clipboard_html, clipboard_text, date_from_unix, fmt_hours, fmt_range, group_commits, merge_rows, month_bounds,
    month_from_unix, month_label, shift_month, total_hours, Estimate, ProjectId, TimesheetCommit,
    TimesheetRow, TimesheetSettings,
};
use gpui::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, Context, Entity, SharedString, WeakEntity,
    Window,
};

use crate::settings::SettingsStore;
use crate::state::{AppState, TimesheetProject};
use crate::text_input::TextInput;
use crate::theme::GlassTheme;
use crate::ui::{back_button, chip, icon, icon_chip, page_header, section_label, ButtonKind, Icon};
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
    settings_store: Entity<SettingsStore>,
    /// Every project with a linked repo; the chips at the top toggle them.
    all_projects: Vec<TimesheetProject>,
    /// The ones switched on, which the tables are built for.
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
    pub fn new(
        cx: &mut Context<Self>,
        state: Entity<AppState>,
        settings_store: Entity<SettingsStore>,
        theme: GlassTheme,
    ) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            theme,
            month: month_from_unix(now_unix()),
            settings: TimesheetSettings::default(),
            settings_store,
            all_projects: Vec::new(),
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

    /// Reads the projects that have a repo and keeps the ones the user has
    /// not switched off for the timesheet.
    fn load_projects(&mut self, cx: &mut Context<Self>) {
        self.all_projects = self.state.read(cx).timesheet_projects();
        let settings = self.settings_store.read(cx).get();
        let order = &settings.timesheet_order;
        // Stable sort: projects the user never arranged keep sidebar order, after the arranged ones.
        self.all_projects
            .sort_by_key(|p| order.iter().position(|id| *id == p.id.0).unwrap_or(usize::MAX));
        let excluded = &settings.timesheet_excluded;
        self.projects = self
            .all_projects
            .iter()
            .filter(|p| !excluded.contains(&p.id.0))
            .cloned()
            .collect();
    }

    /// Moves a project one place earlier (`-1`) or later (`1`) in the
    /// timesheet and in the copied report.
    fn move_project(&mut self, project: ProjectId, delta: i32, cx: &mut Context<Self>) {
        let mut ids: Vec<i64> = self.all_projects.iter().map(|p| p.id.0).collect();
        let Some(at) = ids.iter().position(|id| *id == project.0) else {
            return;
        };
        let to = at as i32 + delta;
        if to < 0 || to as usize >= ids.len() {
            return;
        }
        ids.swap(at, to as usize);
        self.settings_store
            .update(cx, |store, cx| store.update(cx, |s| s.timesheet_order = ids));
        self.load_projects(cx);
        cx.notify();
    }

    /// Switches a project in or out of the timesheet and fills it from git.
    fn toggle_project(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.settings_store.update(cx, |store, cx| {
            store.update(cx, |s| {
                if let Some(at) = s.timesheet_excluded.iter().position(|id| *id == project.0) {
                    s.timesheet_excluded.remove(at);
                } else {
                    s.timesheet_excluded.push(project.0);
                    s.timesheet_excluded.sort_unstable();
                }
            });
        });
        self.refresh(cx);
        self.sync(cx);
    }

    /// Reloads settings, projects and the month from the database. A month
    /// that was never saved is filled from git right away.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        let (settings, month) = {
            let state = self.state.read(cx);
            (
                state.timesheet_settings(),
                state.load_timesheet_month(&self.month),
            )
        };
        self.settings = settings;
        self.load_projects(cx);
        match month {
            Ok(month) => {
                let shown = &self.projects;
                self.tables = month
                    .tables
                    .into_iter()
                    .filter(|(id, _)| shown.iter().any(|p| p.id == *id))
                    .collect();
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
        self.load_projects(cx);
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
        // Second flavour with borders and bold, picked up by Sheets and Excel.
        let html = clipboard_html(&self.settings, &self.month, &self.remarks, &tables);
        let styled = crate::clipboard_html::add_html(&html);
        let label = month_label(&self.month);
        self.set_notice(
            if styled {
                format!(
                    "Copied. In your timesheet add a tab named \"{label}\", click cell A1 and \
                     paste (Ctrl+V). Borders, bold text and the SUM formulas come across."
                )
            } else {
                format!(
                    "Copied as plain text. In your timesheet add a tab named \"{label}\", click \
                     cell A1 and paste (Ctrl+V). Values and the SUM formulas come across; fonts \
                     and borders do not."
                )
            },
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
        crate::ui::button(
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
                        .t_small()
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
                    el.child(div().t_small().text_color(theme.danger).child(message))
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
                                .t_small()
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
            .py(px(3.0))
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .text_color(if empty {
                theme.text_muted
            } else {
                theme.text_primary
            })
            .hover(move |s| s.bg(theme.surface_hover).border_color(theme.border_subtle))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.begin_edit(target.clone(), initial.clone(), window, cx);
            }))
            .child(if empty {
                SharedString::from("Click to edit")
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
            .group("ts-row")
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .py(theme.space_2)
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .child(
                        div()
                            .w(px(24.0))
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child((index + 1).to_string()),
                    )
                    .child(div().flex_1().min_w(px(0.0)).child(title))
                    .child(
                        div()
                            .w(px(150.0))
                            .t_small()
                            .t_code()
                            .text_color(theme.text_secondary)
                            .child(fmt_range(&row.date_from, &row.date_to)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_1)
                            .w(px(170.0))
                            .child(icon_button(&theme, "−", ("ts-minus", uid)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.step_hours(project, &step_key_a, -HOURS_STEP, cx)
                                }),
                            ))
                            .child(div().w(px(56.0)).t_code().child(hours))
                            .child(icon_button(&theme, "+", ("ts-plus", uid)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.step_hours(project, &step_key_b, HOURS_STEP, cx)
                                }),
                            ))
                            .child(div().t_small().text_color(theme.text_muted).child(
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
                        .flex()
                        .items_center()
                        .gap(theme.space_1)
                        .t_small()
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(theme.text_primary))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_commits(project, &toggle_key, cx)
                        }))
                        .child(icon(
                            if open {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                            12.0,
                            theme.text_muted,
                        ))
                        .child(stats),
                )
            })
            .when(open, |el| {
                el.child(
                    div()
                        .ml(px(24.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .p(theme.space_3)
                        .rounded(theme.radius_md)
                        .bg(theme.surface_sunken)
                        .children(row.commits.iter().map(|c| {
                            div()
                                .flex()
                                .gap(theme.space_3)
                                .t_small()
                                .child(
                                    div()
                                        .t_code()
                                        .text_color(theme.accent)
                                        .child(c.hash.chars().take(7).collect::<String>()),
                                )
                                .child(
                                    div()
                                        .t_code()
                                        .text_color(theme.text_muted)
                                        .child(c.date.clone()),
                                )
                                .child(div().text_color(theme.text_secondary).child(c.repo.clone()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_color(theme.text_primary)
                                        .child(c.subject.clone()),
                                )
                        })),
                )
            })
            .into_any_element()
    }

    /// One chip per project with a repo; a lit chip is part of the timesheet.
    fn render_project_toggles(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let excluded = self.settings_store.read(cx).get().timesheet_excluded.clone();
        let chips: Vec<AnyElement> = self
            .all_projects
            .iter()
            .map(|p| {
                let id = p.id;
                div()
                    .flex()
                    .items_center()
                    .child(
                        icon_button(&theme, "‹", ("ts-earlier", id.0 as u64))
                            .on_click(cx.listener(move |this, _, _, cx| this.move_project(id, -1, cx))),
                    )
                    .child(
                        crate::ui::filter_chip(
                            &theme,
                            ("ts-include", id.0 as u64),
                            p.name.clone(),
                            !excluded.contains(&id.0),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(id, cx))),
                    )
                    .child(
                        icon_button(&theme, "›", ("ts-later", id.0 as u64))
                            .on_click(cx.listener(move |this, _, _, cx| this.move_project(id, 1, cx))),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(section_label(&theme, "Include in timesheet (‹ › to reorder)"))
            .child(if chips.is_empty() {
                div()
                    .t_small()
                    .text_color(theme.text_secondary)
                    .child("Only projects with a linked git repo can be added.")
            } else {
                div().flex().flex_wrap().gap(theme.space_2).children(chips)
            })
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
                            .gap(theme.space_3)
                            .child(
                                div()
                                    .t_heading()
                                    .text_color(theme.text_primary)
                                    .child(project.name.clone()),
                            )
                            .child(chip(
                                &theme,
                                format!("{} h", fmt_hours(total_hours(rows))),
                                theme.accent,
                            ))
                            .when(project.repos.is_empty(), |el| {
                                el.child(icon_chip(
                                    &theme,
                                    Icon::Triangle,
                                    "No repos linked",
                                    theme.warning,
                                ))
                            }),
                    )
                    .child(
                        crate::ui::button(
                            &theme,
                            ("ts-add", id.0 as u64),
                            ButtonKind::Ghost,
                            Some(Icon::Plus),
                            "Add task",
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.add_task(id, window, cx)),
                        ),
                    ),
            )
            .child(
                // Column heads.
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .pb(theme.space_1)
                    .border_b_1()
                    .border_color(theme.border_light)
                    .child(div().w(px(24.0)).child(section_label(&theme, "#")))
                    .child(div().flex_1().child(section_label(&theme, "Task")))
                    .child(div().w(px(150.0)).child(section_label(&theme, "Dates")))
                    .child(div().w(px(170.0)).child(section_label(&theme, "Hours")))
                    .child(div().w(px(24.0))),
            )
            .when(rows.is_empty(), |el| {
                el.child(
                    div()
                        .py(theme.space_4)
                        .t_body()
                        .text_color(theme.text_secondary)
                        .child("No commits found this month."),
                )
            })
            .children(rendered)
            .into_any_element()
    }
}

/// A big number over a small label, for the summary strip.
fn summary_stat(theme: &GlassTheme, value: String, label: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .t_title()
                .t_code()
                .text_color(theme.text_primary)
                .child(value),
        )
        .child(
            div()
                .t_small()
                .text_color(theme.text_secondary)
                .child(label.to_string()),
        )
}

impl Render for TimesheetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
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

        let month_switch = div()
            .flex()
            .items_center()
            .child(
                icon_button(&theme, "‹", "ts-prev")
                    .on_click(cx.listener(|this, _, _, cx| this.change_month(-1, cx))),
            )
            .child(
                div()
                    .w(px(132.0))
                    .text_center()
                    .t_body()
                    .t_strong()
                    .text_color(theme.text_primary)
                    .child(month_label(&self.month)),
            )
            .child(
                icon_button(&theme, "›", "ts-next")
                    .on_click(cx.listener(|this, _, _, cx| this.change_month(1, cx))),
            );

        let actions = div()
            .flex()
            .items_center()
            .gap(theme.space_2)
            .child(month_switch)
            .child(
                crate::ui::button(
                    &theme,
                    "ts-sync",
                    ButtonKind::Secondary,
                    Some(Icon::Refresh),
                    if self.syncing {
                        "Syncing…"
                    } else {
                        "Sync from git"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .child(
                crate::ui::button(
                    &theme,
                    "ts-settings",
                    ButtonKind::Ghost,
                    Some(Icon::Settings),
                    "",
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .child(
                crate::ui::button(
                    &theme,
                    "ts-copy",
                    ButtonKind::Primary,
                    Some(Icon::Copy),
                    "Copy all",
                )
                .on_click(cx.listener(|this, _, _, cx| this.copy_all(cx))),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .child(page_header(
                &theme,
                "Timesheet",
                Some("Hours from your git commits, grouped by ticket".into()),
                Some(
                    back_button(&theme, "ts-back")
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(close) = &this.on_close {
                                close(window, cx);
                            }
                        }))
                        .into_any_element(),
                ),
                Some(actions.into_any_element()),
            ))
            .child(
                div()
                    .id("ts-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_6)
                    .px(theme.space_6)
                    .py(theme.space_5)
                    .overflow_y_scroll()
                    .when_some(self.notice.clone(), |el, (message, is_error)| {
                        el.child(crate::ui::notice(&theme, message, is_error))
                    })
                    .when_some(self.render_settings(cx), |el, form| el.child(form))
                    .child(self.render_project_toggles(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_8)
                            .child(summary_stat(
                                &theme,
                                format!("{} h", fmt_hours(hours)),
                                "Total",
                            ))
                            .child(summary_stat(&theme, tasks.to_string(), "Tasks"))
                            .child(summary_stat(&theme, commits.to_string(), "Commits")),
                    )
                    .when(projects.is_empty(), |el| {
                        el.child(crate::ui::empty_state(
                            &theme,
                            Icon::Clock,
                            "No projects yet",
                            "Create a project and link its git repositories to build a timesheet.",
                            None,
                        ))
                    })
                    .children(sections)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space_1)
                            .child(section_label(&theme, "Remarks"))
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
            self.settings_open = false;
            self.run_open = false;
            self.credentials_open = false;
            self.timesheet_open = true;
            self.timesheet_panel
                .update(cx, |panel, cx| panel.refresh(cx));
        }
        cx.notify();
    }

    /// Sidebar row that opens the timesheet.
    pub(crate) fn timesheet_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.nav_row(
            crate::NAV_TIMESHEET,
            self.timesheet_open,
            None,
            |this, cx| this.toggle_timesheet_view(cx),
            cx,
        )
    }
}
