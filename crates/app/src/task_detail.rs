//! Detail page for one task or subtask (any depth): its markdown notes,
//! attached files and mapped git commits, in tabs. Opened from a task row;
//! replaces the main content like the note editor does.

use crate::theme::TypeScale;
use std::collections::HashMap;

use devspace_core::{
    cherry_pick_script, Attachment, AttachmentId, CherryPickCommit, CherryPickGroup, CommitLink,
    CommitLinkId, CommitLinkStatus, Note, NoteId, NoteKind, RepoId, TaskId, TaskPriority,
    TaskStatus,
};
use devspace_git::RepoCommit;
use gpui::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, Context, Entity, Hsla, PathPromptOptions,
    SharedString, Window,
};

use crate::attach_flow::{attach_notice, attach_paths};
use crate::date::{format_date, format_iso_date, parse_date};
use crate::external::{open_attachment, reveal, OpenOutcome};
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;
use crate::ui::{
    back_button, button, chip, empty_state, icon, icon_button, icon_chip, notice, section_label,
    segment, segmented, status_style, tab, tab_bar, ButtonKind, Icon,
};

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;
pub type OpenNoteHandler = Box<dyn Fn(NoteId, SharedString, &mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Notes,
    Attachments,
    Commits,
}

/// Commits listed in the branch view, newest first.
const BRANCH_LIMIT: usize = 100;
/// How far back (per repo) to look for commits that mention the task.
const SUGGEST_SCAN: usize = 2000;
const SUGGEST_MAX: usize = 20;

type CommitKey = (RepoId, String);
/// What the branch/suggestion lookups were computed for.
type ExtrasKey = (Option<String>, Vec<RepoId>, Vec<CommitKey>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Due,
    Branch,
}

impl TaskDetail {
    /// Switches tab by name (debug QA hook; unknown names change nothing).
    pub(crate) fn show_tab(&mut self, name: &str, cx: &mut Context<Self>) {
        self.tab = match name {
            "notes" => Tab::Notes,
            "attachments" => Tab::Attachments,
            "commits" => Tab::Commits,
            _ => return,
        };
        cx.notify();
    }

    /// The task this page shows (remembered across restarts, see settings).
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }
}

pub struct TaskDetail {
    state: Entity<AppState>,
    theme: GlassTheme,
    task_id: TaskId,
    tab: Tab,
    new_note_input: Entity<TextInput>,
    /// "Add tag…" input on the task page; Enter adds the tag.
    tag_input: Entity<TextInput>,
    /// Copy picked files into the workspace (default) vs link by path.
    copy_files: bool,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    /// Git details for this task's mapped commits, read in the background.
    /// `None` inside means git could not find the commit any more.
    commit_info: HashMap<CommitKey, Option<RepoCommit>>,
    commit_keys_requested: Vec<CommitKey>,
    /// The due-date / branch field currently being edited, with its input.
    editing: Option<(Field, Entity<TextInput>)>,
    /// Per repo: the commits on this task's branch that are not in the
    /// target branch, and commits that look like they belong to the task.
    /// Both are read in the background and refreshed when their inputs
    /// (branch, linked commits, repos) change.
    branch_views: Vec<(RepoId, String, devspace_git::BranchView)>,
    suggestions: Vec<RepoCommit>,
    extras_key: Option<ExtrasKey>,

    pub on_close: Option<CloseHandler>,
    pub on_open_note: Option<OpenNoteHandler>,
    /// Jump to the commit browser to map more commits.
    pub on_open_history: Option<CloseHandler>,
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "Todo",
        TaskStatus::InProgress => "In Progress",
        TaskStatus::Blocked => "Blocked",
        TaskStatus::Done => "Done",
    }
}

fn link_status_label(status: CommitLinkStatus) -> &'static str {
    match status {
        CommitLinkStatus::Linked => "Linked",
        CommitLinkStatus::NeedsMigration => "Needs migration",
        CommitLinkStatus::CherryPicked => "Cherry-picked",
        CommitLinkStatus::Deployed => "Deployed",
        CommitLinkStatus::Missing => "Missing",
    }
}

fn link_status_color(theme: &GlassTheme, status: CommitLinkStatus) -> Hsla {
    match status {
        CommitLinkStatus::Linked | CommitLinkStatus::CherryPicked => theme.accent,
        CommitLinkStatus::NeedsMigration => theme.warning,
        CommitLinkStatus::Deployed => theme.success,
        CommitLinkStatus::Missing => theme.danger,
    }
}

impl TaskDetail {
    pub fn new(
        cx: &mut Context<Self>,
        state: Entity<AppState>,
        theme: GlassTheme,
        task_id: TaskId,
    ) -> Self {
        let new_note_input = cx.new(|cx| TextInput::new(cx, "New note title…", theme));
        let me = cx.entity();
        new_note_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |title, window, cx| {
                let title = title.trim().to_string();
                if title.is_empty() {
                    return;
                }
                me.update(cx, |this, cx| this.create_note(&title, window, cx));
            }));
        });

        let tag_input = cx.new(|cx| TextInput::new(cx, "Add tag…", theme));
        let me_for_tags = cx.entity();
        tag_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |tag, _window, cx| {
                let tag = tag.to_string();
                me_for_tags.update(cx, |this, cx| this.add_tag(&tag, cx));
            }));
        });

        // Links/tasks/notes change from elsewhere (commit browser, deletes).
        cx.observe(&state, |this, _, cx| {
            this.refresh_commits(cx);
            cx.notify();
        })
        .detach();

        Self {
            state,
            theme,
            task_id,
            tab: Tab::Notes,
            new_note_input,
            tag_input,
            copy_files: true,
            notice: None,
            commit_info: HashMap::new(),
            commit_keys_requested: Vec::new(),
            editing: None,
            branch_views: Vec::new(),
            suggestions: Vec::new(),
            extras_key: None,
            on_close: None,
            on_open_note: None,
            on_open_history: None,
        }
    }

    /// Swaps the due-date or branch label for a text input prefilled with
    /// the current value. Enter saves (empty clears), Escape cancels.
    fn start_edit(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let current = {
            let state = self.state.read(cx);
            let task = state.tasks.iter().find(|t| t.id == self.task_id);
            match field {
                Field::Due => task
                    .and_then(|t| t.due_at)
                    .map(format_iso_date)
                    .unwrap_or_default(),
                Field::Branch => task.and_then(|t| t.branch.clone()).unwrap_or_default(),
            }
        };
        let placeholder = match field {
            Field::Due => "YYYY-MM-DD (empty clears)",
            Field::Branch => "Branch name (empty clears)",
        };
        let theme = self.theme;
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, placeholder, theme);
            input.content = current.into();
            input
        });
        let me = cx.entity();
        input.update(cx, |input, _| {
            let confirm = me.clone();
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                confirm.update(cx, |this, cx| this.apply_field(field, &text, cx));
            }));
            input.on_cancel = Some(Box::new(move |_window, cx| {
                me.update(cx, |this, cx| {
                    this.editing = None;
                    cx.notify();
                });
            }));
        });
        window.focus(&input.read(cx).focus_handle.clone(), cx);
        self.notice = None;
        self.editing = Some((field, input));
        cx.notify();
    }

    fn apply_field(&mut self, field: Field, text: &str, cx: &mut Context<Self>) {
        let task = self.task_id;
        let text = text.trim();
        match field {
            Field::Due => {
                let due = if text.is_empty() {
                    None
                } else {
                    match parse_date(text) {
                        Some(ts) => Some(ts),
                        None => {
                            // Keep the input open so the typo can be fixed.
                            self.notice =
                                Some(("Use a real date like 2026-10-30".to_string(), true));
                            cx.notify();
                            return;
                        }
                    }
                };
                self.state.update(cx, |state, cx| {
                    state.set_task_due(task, due);
                    cx.notify();
                });
            }
            Field::Branch => {
                let branch = (!text.is_empty()).then(|| text.to_string());
                self.state.update(cx, |state, cx| {
                    state.set_task_branch(task, branch);
                    cx.notify();
                });
            }
        }
        self.notice = None;
        self.editing = None;
        cx.notify();
    }

    fn add_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if tag.trim().is_empty() {
            return;
        }
        let task = self.task_id;
        let result = self.state.update(cx, |state, cx| {
            let r = state.add_task_tag(task, tag);
            cx.notify();
            r
        });
        self.notice = match result {
            Ok(true) => None,
            Ok(false) => Some(("That tag is already on this task".into(), true)),
            Err(message) => Some((message, true)),
        };
        cx.notify();
    }

    /// The task's tags as removable chips, plus the "Add tag…" input.
    fn tags_row(&self, task: &devspace_core::Task, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.space_2)
            .children(task.tags.iter().enumerate().map(|(ix, tag)| {
                let name = tag.clone();
                let tint = crate::theme::label_color(&theme, tag);
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .h(px(26.0))
                    .pl(theme.space_3)
                    .pr(px(3.0))
                    .rounded(theme.radius_sm)
                    .bg(tint.opacity(0.14))
                    .t_small()
                    .t_strong()
                    .text_color(tint)
                    .child(tag.clone())
                    .child(
                        icon_button(&theme, Icon::X, ("remove-tag", ix as u64), theme.danger)
                            .size(px(20.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = this.task_id;
                                let name = name.clone();
                                this.state.update(cx, |state, cx| {
                                    state.remove_task_tag(id, &name);
                                    cx.notify();
                                });
                            })),
                    )
            }))
            .child(div().w(px(160.0)).child(self.tag_input.clone()))
            .into_any_element()
    }

    /// Priority, due date and branch, each editable in place.
    fn details_row(&self, task: &devspace_core::Task, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let priority_color = match task.priority {
            TaskPriority::Urgent => theme.danger,
            TaskPriority::High => theme.warning,
            TaskPriority::Medium => theme.text_primary,
            TaskPriority::Low => theme.text_secondary,
        };
        let priority_label = match task.priority {
            TaskPriority::Low => "Low",
            TaskPriority::Medium => "Medium",
            TaskPriority::High => "High",
            TaskPriority::Urgent => "Urgent",
        };
        // Overdue = due date is before the start of today (UTC) and unfinished.
        let overdue = task.status != TaskStatus::Done
            && task
                .due_at
                .is_some_and(|due| due < now - now.rem_euclid(86_400));

        // One property cell: a micro label over the value.
        let cell = |label: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .px(theme.space_3)
                .py(theme.space_2)
                .rounded(theme.radius_md)
                .child(section_label(&theme, label))
        };

        let field = |label: &'static str,
                     value: Option<String>,
                     which: Field,
                     color: Hsla,
                     cx: &mut Context<Self>|
         -> AnyElement {
            let editing_here = self
                .editing
                .as_ref()
                .filter(|(f, _)| *f == which)
                .map(|(_, input)| input.clone());
            match editing_here {
                Some(input) => cell(label)
                    .child(div().w(px(220.0)).child(input))
                    .into_any_element(),
                None => cell(label)
                    .id(match which {
                        Field::Due => "edit-due",
                        Field::Branch => "edit-branch",
                    })
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_edit(which, window, cx);
                    }))
                    .child(match value {
                        Some(v) => div()
                            .t_body()
                            .when(which == Field::Branch, |d| d.t_code())
                            .text_color(color)
                            .child(v),
                        None => div().t_body().text_color(theme.text_muted).child("Not set"),
                    })
                    .into_any_element(),
            }
        };

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.space_2)
            .child(
                cell("Priority")
                    .id("cycle-priority")
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let id = this.task_id;
                        this.state.update(cx, |state, cx| {
                            state.cycle_task_priority(id);
                            cx.notify();
                        });
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .t_body()
                            .text_color(priority_color)
                            .child(div().size(px(8.0)).rounded_full().bg(priority_color))
                            .child(priority_label),
                    ),
            )
            .child(field(
                if overdue { "Overdue" } else { "Due" },
                task.due_at.map(format_date),
                Field::Due,
                if overdue {
                    theme.danger
                } else {
                    theme.text_primary
                },
                cx,
            ))
            .child(field(
                "Branch",
                task.branch.clone(),
                Field::Branch,
                theme.text_primary,
                cx,
            ))
            .into_any_element()
    }

    fn create_note(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let task = self.task_id;
        let created = self.state.update(cx, |state, cx| {
            let id = state.create_note_for(Some(task), title);
            cx.notify();
            id
        });
        match created {
            Some(id) => {
                if let Some(open) = &self.on_open_note {
                    open(id, title.to_string().into(), window, cx);
                }
            }
            None => self.notice = Some(("Couldn't create the note".into(), true)),
        }
        cx.notify();
    }

    /// Loads the branch view and suggested commits in the background. Skips
    /// the work when nothing it depends on changed (the task's branch, the
    /// linked repos, or the set of already-mapped commits).
    fn refresh_extras(&mut self, cx: &mut Context<Self>) {
        let (branch, repos, linked) = {
            let state = self.state.read(cx);
            let branch = state
                .tasks
                .iter()
                .find(|t| t.id == self.task_id)
                .and_then(|t| t.branch.clone());
            let linked: Vec<CommitKey> = state
                .commit_links
                .iter()
                .filter(|l| l.task_id == self.task_id)
                .map(|l| (l.repo_id, l.hash.clone()))
                .collect();
            (branch, state.repos.clone(), linked)
        };
        let key: ExtrasKey = (
            branch.clone(),
            repos.iter().map(|r| r.id).collect(),
            linked.clone(),
        );
        if self.extras_key.as_ref() == Some(&key) {
            return;
        }
        self.extras_key = Some(key.clone());
        let task_id = self.task_id;
        cx.spawn(async move |this, cx| {
            let (views, suggestions) = cx
                .background_executor()
                .spawn(async move {
                    let views: Vec<(RepoId, String, devspace_git::BranchView)> = match branch
                        .as_deref()
                    {
                        Some(name) => repos
                            .iter()
                            .filter_map(|repo| {
                                devspace_git::branch_only_commits(repo, name, None, BRANCH_LIMIT)
                                    .ok()
                                    .map(|v| (repo.id, repo.name.clone(), v))
                            })
                            .collect(),
                        None => Vec::new(),
                    };
                    let already: std::collections::HashSet<CommitKey> =
                        linked.into_iter().collect();
                    let suggestions = devspace_git::suggested_for_task(
                        &repos,
                        task_id,
                        branch.as_deref(),
                        &already,
                        SUGGEST_SCAN,
                        SUGGEST_MAX,
                    );
                    (views, suggestions)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Ignore a result for inputs that have since changed.
                if this.extras_key.as_ref() != Some(&key) {
                    return;
                }
                this.branch_views = views;
                this.suggestions = suggestions;
                cx.notify();
            });
        })
        .detach();
    }

    /// Maps one commit to this task as a plain "Linked" mapping.
    fn map_commit(&mut self, repo: RepoId, hash: &str, cx: &mut Context<Self>) {
        let task = self.task_id;
        let commits = vec![(repo, hash.to_string())];
        let result = self.state.update(cx, |state, cx| {
            let r = state.link_commits(task, &commits, CommitLinkStatus::Linked, None);
            cx.notify();
            r
        });
        self.notice = Some(match result {
            Ok(_) => ("Mapped to this task".into(), false),
            Err(message) => (message, true),
        });
        cx.notify();
    }

    /// Reads git details for this task's mapped commits on a background
    /// task whenever the set of mapped commits changes.
    fn refresh_commits(&mut self, cx: &mut Context<Self>) {
        self.refresh_extras(cx);
        let (keys, repos) = {
            let state = self.state.read(cx);
            let keys: Vec<CommitKey> = state
                .commit_links
                .iter()
                .filter(|l| l.task_id == self.task_id)
                .map(|l| (l.repo_id, l.hash.clone()))
                .collect();
            (keys, state.repos.clone())
        };
        if keys == self.commit_keys_requested {
            return;
        }
        self.commit_keys_requested = keys.clone();
        let wanted = keys.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { devspace_git::lookup_commits(&repos, &wanted) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Ignore if the mapped set moved on while we were reading.
                if this.commit_keys_requested != keys {
                    return;
                }
                this.commit_info = found.into_iter().collect();
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies shell text that cherry-picks this task's commits (oldest
    /// first, grouped per repo) to the clipboard. Uses the git details the
    /// tab already loaded, so a commit git can't find is listed as missing
    /// instead of silently dropped.
    fn copy_cherry_pick(&mut self, cx: &mut Context<Self>) {
        let (links, repos) = {
            let state = self.state.read(cx);
            let links: Vec<CommitLink> = state
                .commit_links
                .iter()
                .filter(|l| l.task_id == self.task_id)
                .cloned()
                .collect();
            (links, state.repos.clone())
        };
        let still_loading = links
            .iter()
            .any(|l| !self.commit_info.contains_key(&(l.repo_id, l.hash.clone())));
        if still_loading {
            self.notice = Some((
                "Still reading the commits from git — try again".into(),
                true,
            ));
            cx.notify();
            return;
        }
        let groups: Vec<CherryPickGroup> = repos
            .iter()
            .filter_map(|repo| {
                let commits: Vec<CherryPickCommit> = links
                    .iter()
                    .filter(|l| l.repo_id == repo.id)
                    .map(|l| {
                        let found = self
                            .commit_info
                            .get(&(l.repo_id, l.hash.clone()))
                            .and_then(|c| c.as_ref());
                        CherryPickCommit {
                            hash: l.hash.clone(),
                            summary: found.map(|c| c.commit.summary.clone()),
                            time: found.map(|c| c.commit.time),
                        }
                    })
                    .collect();
                (!commits.is_empty()).then(|| CherryPickGroup {
                    repo_name: repo.name.clone(),
                    repo_path: repo.path.clone(),
                    commits,
                })
            })
            .collect();
        let script = cherry_pick_script(&groups);
        if script.is_empty() {
            self.notice = Some(("Nothing to export".into(), true));
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(script));
            self.notice = Some((
                format!(
                    "Copied the cherry-pick list for {} commit{} — paste it in a terminal \
                     inside each repo",
                    links.len(),
                    if links.len() == 1 { "" } else { "s" }
                ),
                false,
            ));
        }
        cx.notify();
    }

    fn add_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        let state = self.state.clone();
        let task = self.task_id;
        let copy = self.copy_files;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let (added, failure) = attach_paths(&state, Some(task), paths, copy, cx).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(attach_notice(added, failure));
                cx.notify();
            });
        })
        .detach();
    }

    fn open_file(&mut self, attachment: &Attachment, cx: &mut Context<Self>) {
        let Some(path) = self.state.read(cx).attachment_path(attachment) else {
            self.notice = Some(("That file is missing or outside the workspace".into(), true));
            cx.notify();
            return;
        };
        self.notice = Some(match open_attachment(&path, &attachment.name) {
            Ok(OpenOutcome::Opened) => (format!("Opened {}", attachment.name), false),
            Ok(OpenOutcome::Revealed) => (
                "Shown in the folder — programs and scripts are never launched from here".into(),
                false,
            ),
            Err(message) => (message, true),
        });
        cx.notify();
    }

    fn reveal_file(&mut self, attachment: &Attachment, cx: &mut Context<Self>) {
        let Some(path) = self.state.read(cx).attachment_path(attachment) else {
            self.notice = Some(("That file is missing or outside the workspace".into(), true));
            cx.notify();
            return;
        };
        if let Err(message) = reveal(&path) {
            self.notice = Some((message, true));
        }
        cx.notify();
    }

    /// A flat list row with a hairline under it. Its action area (see
    /// [`row_actions`]) shows while the row is hovered.
    fn row(&self) -> gpui::Div {
        let theme = self.theme;
        div()
            .group("row")
            .flex()
            .items_center()
            .gap(theme.space_3)
            .min_h(px(48.0))
            .px(theme.space_2)
            .py(theme.space_2)
            .border_b_1()
            .border_color(theme.border_subtle)
    }

    fn empty(&self, glyph: Icon, title: &'static str, hint: &'static str) -> AnyElement {
        empty_state(&self.theme, glyph, title, hint, None).into_any_element()
    }

    fn section_header(&self, label: String) -> AnyElement {
        let theme = self.theme;
        div()
            .mt(theme.space_5)
            .pb(theme.space_1)
            .child(section_label(&theme, &label))
            .into_any_element()
    }

    fn notes_tab(&mut self, notes: Vec<Note>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme;
        let mut out: Vec<AnyElement> =
            vec![div().child(self.new_note_input.clone()).into_any_element()];
        out.push(
            div()
                .px(theme.space_1)
                .t_small()
                .text_color(theme.text_muted)
                .child("Notes are markdown. Enter creates the note and opens it.")
                .into_any_element(),
        );
        if notes.is_empty() {
            out.push(self.empty(
                Icon::StickyNote,
                "No notes on this task",
                "Notes are markdown files: write down decisions, scripts or SQL for this task.",
            ));
        }
        let rows = notes.into_iter().map(|note| {
            let id = note.id;
            let title: SharedString = note.title.clone().into();
            self.row()
                .id(("task-note", id.0 as u64))
                .cursor_pointer()
                .hover(move |s| s.bg(theme.surface_hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(open) = &this.on_open_note {
                        open(id, title.clone(), window, cx);
                    }
                }))
                .child(icon(note_icon(note.kind), 16.0, theme.text_muted))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .t_body()
                                .text_color(theme.text_primary)
                                .truncate()
                                .child(note.title.clone()),
                        )
                        .child(div().t_small().t_code().text_color(theme.text_muted).child(
                            match note.kind {
                                NoteKind::Note => {
                                    format!("updated {}", format_date(note.updated_at))
                                }
                                kind => format!(
                                    "{} · updated {}",
                                    kind.label().to_lowercase(),
                                    format_date(note.updated_at)
                                ),
                            },
                        )),
                )
                .child(
                    row_actions().child(
                        icon_button(
                            &theme,
                            Icon::Trash,
                            ("task-note-delete", id.0 as u64),
                            theme.danger,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.delete_note(id);
                                cx.notify();
                            });
                            cx.stop_propagation();
                        })),
                    ),
                )
                .into_any_element()
        });
        out.push(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme.border_subtle)
                .children(rows.collect::<Vec<_>>())
                .into_any_element(),
        );
        out
    }

    fn attachments_tab(
        &mut self,
        attachments: Vec<Attachment>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = self.theme;
        let mut out: Vec<AnyElement> = vec![div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.space_3)
            .child(
                button(
                    &theme,
                    "attach-files",
                    ButtonKind::Secondary,
                    Some(Icon::Plus),
                    "Add files",
                )
                .on_click(cx.listener(|this, _, _, cx| this.add_files(cx))),
            )
            .child(
                segmented(&theme)
                    .child(
                        segment(
                            &theme,
                            "attach-copy",
                            "Copy into workspace",
                            self.copy_files,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_files = true;
                            cx.notify();
                        })),
                    )
                    .child(
                        segment(&theme, "attach-link", "Link by path", !self.copy_files).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.copy_files = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .into_any_element()];
        out.push(
            div()
                .px(theme.space_1)
                .t_small()
                .text_color(theme.text_muted)
                .child(if self.copy_files {
                    "Files are copied into this project's folder and stay available even if the original moves."
                } else {
                    "Files stay where they are; Devspace only remembers the path."
                })
                .into_any_element(),
        );
        if attachments.is_empty() {
            out.push(self.empty(
                Icon::Paperclip,
                "No files attached",
                "Attach specs, screenshots or exports that belong to this task.",
            ));
        }
        let rows = attachments.into_iter().map(|attachment| {
            let id: AttachmentId = attachment.id;
            let for_open = attachment.clone();
            let for_reveal = attachment.clone();
            self.row()
                .id(("attachment", id.0 as u64))
                .cursor_pointer()
                .hover(move |s| s.bg(theme.surface_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.open_file(&for_open, cx)))
                .child(icon(
                    if attachment.is_linked {
                        Icon::Link
                    } else {
                        Icon::Paperclip
                    },
                    16.0,
                    theme.text_muted,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .t_body()
                                .text_color(theme.text_primary)
                                .truncate()
                                .child(attachment.name.clone()),
                        )
                        .child(div().t_small().t_code().text_color(theme.text_muted).child(
                            format!(
                                "{} · {}",
                                if attachment.is_linked {
                                    "linked"
                                } else {
                                    "copied"
                                },
                                format_date(attachment.created_at)
                            ),
                        )),
                )
                .child(
                    row_actions()
                        .child(
                            icon_button(
                                &theme,
                                Icon::FolderOpen,
                                ("attachment-reveal", id.0 as u64),
                                theme.text_primary,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.reveal_file(&for_reveal, cx);
                                    cx.stop_propagation();
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                &theme,
                                Icon::Trash,
                                ("attachment-remove", id.0 as u64),
                                theme.danger,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.remove_attachment(id);
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                )
                .into_any_element()
        });
        out.push(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme.border_subtle)
                .children(rows.collect::<Vec<_>>())
                .into_any_element(),
        );
        out
    }

    fn commits_tab(&mut self, links: Vec<CommitLink>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme;
        let has_links = !links.is_empty();
        let mut out: Vec<AnyElement> = vec![div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(theme.space_3)
            .child(
                div()
                    .t_small()
                    .text_color(theme.text_muted)
                    .child("Commits mapped to this task, in the order they were linked."),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .when(has_links, |d| {
                        d.child(
                            button(
                                &theme,
                                "copy-cherry-pick",
                                ButtonKind::Secondary,
                                Some(Icon::Copy),
                                "Copy cherry-pick list",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.copy_cherry_pick(cx))),
                        )
                    })
                    .child(
                        button(
                            &theme,
                            "open-history-from-task",
                            ButtonKind::Primary,
                            Some(Icon::GitCommit),
                            "Map commits",
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(open) = &this.on_open_history {
                                open(window, cx);
                            }
                        })),
                    ),
            )
            .into_any_element()];
        if !has_links {
            out.push(self.empty(
                Icon::GitCommit,
                "No commits mapped yet",
                "Open Commit history, select commits and map them to this task.",
            ));
        }
        let link_rows: Vec<AnyElement> = links
            .into_iter()
            .map(|link| {
                let id: CommitLinkId = link.id;
                let color = link_status_color(&theme, link.status);
                let info = self
                    .commit_info
                    .get(&(link.repo_id, link.hash.clone()))
                    .cloned();
                let (repo, summary, meta, missing) = match info {
                    Some(Some(entry)) => (
                        Some(entry.repo_name),
                        entry.commit.summary,
                        format!(
                            "{} · {}",
                            entry.commit.author,
                            format_date(entry.commit.time)
                        ),
                        false,
                    ),
                    // Lookup finished and git doesn't have it: rebased/squashed
                    // away, or the repo moved. Warn rather than show stale data.
                    Some(None) => (
                        None,
                        "Commit not found in the repository".to_string(),
                        "It may have been rebased or squashed, or the repo was moved".to_string(),
                        true,
                    ),
                    None => (None, "Loading…".to_string(), String::new(), false),
                };
                self.row()
                    .child(icon(
                        if missing {
                            Icon::Triangle
                        } else {
                            Icon::GitCommit
                        },
                        16.0,
                        if missing {
                            theme.warning
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
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_2)
                                    .when_some(repo, |d, name| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .px(theme.space_2)
                                                .rounded(theme.radius_sm)
                                                .bg(theme.surface_hover)
                                                .t_small()
                                                .t_code()
                                                .text_color(theme.text_secondary)
                                                .child(name),
                                        )
                                    })
                                    .child(
                                        div()
                                            .t_body()
                                            .truncate()
                                            .text_color(if missing {
                                                theme.warning
                                            } else {
                                                theme.text_primary
                                            })
                                            .child(summary),
                                    ),
                            )
                            .child(div().t_small().t_code().text_color(theme.text_muted).child(
                                format!(
                                    "{}{}{}",
                                    &link.hash[..7.min(link.hash.len())],
                                    if meta.is_empty() { "" } else { " · " },
                                    meta
                                ),
                            ))
                            .when_some(link.note.clone(), |d, note| {
                                d.child(
                                    div().t_small().text_color(theme.text_secondary).child(note),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                chip(&theme, link_status_label(link.status), color)
                                    .id(("task-link-status", id.0 as u64))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.cycle_link_status(id);
                                            cx.notify();
                                        });
                                    })),
                            )
                            .child(
                                row_actions().child(
                                    icon_button(
                                        &theme,
                                        Icon::X,
                                        ("task-link-remove", id.0 as u64),
                                        theme.danger,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.unlink_commit(id);
                                                cx.notify();
                                            });
                                        },
                                    )),
                                ),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        if !link_rows.is_empty() {
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(theme.border_subtle)
                    .children(link_rows)
                    .into_any_element(),
            );
        }

        // Commits already mapped to this task, to mark them in the lists below.
        let mapped: std::collections::HashSet<CommitKey> = self
            .state
            .read(cx)
            .commit_links
            .iter()
            .filter(|l| l.task_id == self.task_id)
            .map(|l| (l.repo_id, l.hash.clone()))
            .collect();

        // Branch view: what the task's branch has that the target doesn't.
        let branch = self
            .state
            .read(cx)
            .tasks
            .iter()
            .find(|t| t.id == self.task_id)
            .and_then(|t| t.branch.clone());
        match branch {
            None => out.push(
                div()
                    .mt(theme.space_3)
                    .t_small()
                    .text_color(theme.text_muted)
                    .child(
                        "Set a branch on this task (Branch, above) to see the commits on it \
                         that are not in your main branch.",
                    )
                    .into_any_element(),
            ),
            Some(name) => {
                out.push(self.section_header(format!("On branch {name}")));
                let views = self.branch_views.clone();
                let mut ix = 0usize;
                for (repo_id, repo_name, view) in views {
                    if !view.branch_found {
                        out.push(
                            div()
                                .t_small()
                                .text_color(theme.text_secondary)
                                .child(format!("{repo_name}: branch not found"))
                                .into_any_element(),
                        );
                        continue;
                    }
                    let target = view
                        .target
                        .clone()
                        .unwrap_or_else(|| "(no target found)".into());
                    out.push(
                        div()
                            .t_small()
                            .text_color(theme.text_secondary)
                            .child(format!(
                                "{repo_name}: {} commit{} not in {target}{}",
                                view.commits.len(),
                                if view.commits.len() == 1 { "" } else { "s" },
                                if view.truncated {
                                    " (showing the newest)"
                                } else {
                                    ""
                                }
                            ))
                            .into_any_element(),
                    );
                    for commit in view.commits {
                        ix += 1;
                        let is_mapped = mapped.contains(&(repo_id, commit.hash.clone()));
                        out.push(self.candidate_row(
                            Some(repo_id),
                            &repo_name,
                            &commit,
                            is_mapped,
                            ("branch-map", ix as u64),
                            cx,
                        ));
                    }
                }
            }
        }

        // Suggestions: commits that mention this task (#id) or its branch.
        let suggestions = self.suggestions.clone();
        if !suggestions.is_empty() {
            out.push(self.section_header("Suggested: mention this task or its branch".into()));
            for (ix, entry) in suggestions.iter().enumerate() {
                out.push(self.candidate_row(
                    Some(entry.repo_id),
                    &entry.repo_name,
                    &entry.commit,
                    false,
                    ("suggest-map", ix as u64),
                    cx,
                ));
            }
        }
        out
    }

    /// A commit that is not (necessarily) mapped yet, with a one-click Map.
    fn candidate_row(
        &self,
        repo_id: Option<RepoId>,
        repo_name: &str,
        commit: &devspace_git::CommitInfo,
        mapped: bool,
        id: (&'static str, u64),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let hash = commit.hash.clone();
        self.row()
            .child(icon(Icon::GitCommit, 16.0, theme.text_muted))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                div()
                                    .flex_none()
                                    .px(theme.space_2)
                                    .rounded(theme.radius_sm)
                                    .bg(theme.surface_hover)
                                    .t_small()
                                    .t_code()
                                    .text_color(theme.text_secondary)
                                    .child(repo_name.to_string()),
                            )
                            .child(
                                div()
                                    .t_body()
                                    .truncate()
                                    .text_color(theme.text_primary)
                                    .child(commit.summary.clone()),
                            ),
                    )
                    .child(
                        div()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(format!(
                                "{} · {} · {}",
                                &commit.hash[..7.min(commit.hash.len())],
                                commit.author,
                                format_date(commit.time)
                            )),
                    ),
            )
            .child(match (mapped, repo_id) {
                (true, _) => {
                    icon_chip(&theme, Icon::Check, "Mapped", theme.success).into_any_element()
                }
                (false, Some(repo_id)) => button(
                    &theme,
                    id,
                    ButtonKind::Secondary,
                    Some(Icon::Plus),
                    "Map to task",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.map_commit(repo_id, &hash, cx);
                }))
                .into_any_element(),
                (false, None) => div().into_any_element(),
            })
            .into_any_element()
    }
}

/// The hover-only action area at the right of a [`TaskDetail::row`].
fn row_actions() -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .invisible()
        .group_hover("row", |s| s.visible())
}

/// Icon for a note kind.
fn note_icon(kind: NoteKind) -> Icon {
    match kind {
        NoteKind::Note => Icon::FileText,
        NoteKind::Script => Icon::Script,
        NoteKind::Sql => Icon::Database,
    }
}

impl Render for TaskDetail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        // Kick off the git lookup the first time the commits tab is needed
        // (and whenever the mapped set changes, via the state observer).
        if self.tab == Tab::Commits {
            self.refresh_commits(cx);
        }

        let (task, trail, notes, attachments, links) = {
            let state = self.state.read(cx);
            let task = state.tasks.iter().find(|t| t.id == self.task_id).cloned();
            // Ancestors, root first, for the breadcrumb.
            let mut trail: Vec<String> = Vec::new();
            let mut parent = task.as_ref().and_then(|t| t.parent_id);
            while let Some(pid) = parent {
                let Some(p) = state.tasks.iter().find(|t| t.id == pid) else {
                    break;
                };
                trail.push(p.title.clone());
                parent = p.parent_id;
            }
            trail.reverse();
            let links: Vec<CommitLink> = state
                .commit_links
                .iter()
                .filter(|l| l.task_id == self.task_id)
                .cloned()
                .collect();
            (
                task,
                trail,
                state.notes_for_task(self.task_id),
                state.attachments_for_task(self.task_id),
                links,
            )
        };

        let Some(task) = task else {
            // Deleted while open (or project switched): offer the way out.
            return div()
                .flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .child(empty_state(
                    &theme,
                    Icon::Info,
                    "This task no longer exists",
                    "It was deleted, or you switched projects.",
                    Some(
                        button(
                            &theme,
                            "task-gone-back",
                            ButtonKind::Secondary,
                            Some(Icon::ArrowLeft),
                            "Back to project",
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(close) = &this.on_close {
                                close(window, cx);
                            }
                        }))
                        .into_any_element(),
                    ),
                ))
                .into_any_element();
        };

        let counts = (notes.len(), attachments.len(), links.len());
        let body = match self.tab {
            Tab::Notes => self.notes_tab(notes, cx),
            Tab::Attachments => self.attachments_tab(attachments, cx),
            Tab::Commits => self.commits_tab(links, cx),
        };
        let (status_glyph, status_color) = status_style(&theme, task.status);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .min_h(px(0.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(theme.space_4)
                    .px(theme.space_6)
                    .pt(theme.space_5)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_3)
                            .child(back_button(&theme, "task-back").on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let Some(close) = &this.on_close {
                                        close(window, cx);
                                    }
                                },
                            )))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .when(!trail.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .t_small()
                                                .text_color(theme.text_muted)
                                                .truncate()
                                                .child(trail.join("  ›  ")),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .gap(theme.space_2)
                                            .child(
                                                div()
                                                    .t_title()
                                                    .text_color(theme.text_primary)
                                                    .truncate()
                                                    .child(task.title.clone()),
                                            )
                                            .child(
                                                // Commit messages reference a task as #<id>.
                                                div()
                                                    .t_small()
                                                    .t_code()
                                                    .text_color(theme.text_muted)
                                                    .child(format!("#{}", task.id.0)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("task-detail-status")
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(theme.space_2)
                                    .h(px(28.0))
                                    .px(theme.space_3)
                                    .rounded(theme.radius_sm)
                                    .bg(status_color.opacity(0.14))
                                    .text_color(status_color)
                                    .t_body()
                                    .t_strong()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let id = this.task_id;
                                        this.state.update(cx, |state, cx| {
                                            state.cycle_task_status(id);
                                            cx.notify();
                                        });
                                    }))
                                    .child(icon(status_glyph, 14.0, status_color))
                                    .child(status_label(task.status)),
                            ),
                    )
                    .child(self.details_row(&task, cx))
                    .child(self.tags_row(&task, cx))
                    .child(
                        tab_bar(&theme)
                            .child(
                                tab(
                                    &theme,
                                    "tab-notes",
                                    "Notes",
                                    Some(counts.0),
                                    self.tab == Tab::Notes,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.tab = Tab::Notes;
                                        this.notice = None;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                tab(
                                    &theme,
                                    "tab-attachments",
                                    "Attachments",
                                    Some(counts.1),
                                    self.tab == Tab::Attachments,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.tab = Tab::Attachments;
                                        this.notice = None;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                tab(
                                    &theme,
                                    "tab-commits",
                                    "Commits",
                                    Some(counts.2),
                                    self.tab == Tab::Commits,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.tab = Tab::Commits;
                                        this.notice = None;
                                        this.refresh_commits(cx);
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .when_some(self.notice.clone(), |d, (message, is_error)| {
                d.child(
                    div()
                        .px(theme.space_6)
                        .pt(theme.space_3)
                        .child(notice(&theme, message, is_error)),
                )
            })
            .child(
                div()
                    .id("task-detail-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_3)
                    .px(theme.space_6)
                    .py(theme.space_4)
                    .overflow_y_scroll()
                    .children(body),
            )
            .into_any_element()
    }
}
