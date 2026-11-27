//! Detail page for one task or subtask (any depth): its markdown notes,
//! attached files and mapped git commits, in tabs. Opened from a task row;
//! replaces the main content like the note editor does.

use crate::theme::TypeScale;
use std::collections::HashMap;

use devspace_core::{
    cherry_pick_script, Attachment, AttachmentId, CherryPickCommit, CherryPickGroup, CommitLink,
    CommitLinkId, CommitLinkStatus, NoteId, NoteKind, RepoId, Task, TaskId, TaskPriority,
    TaskStatus,
};
use devspace_git::RepoCommit;
use devspace_github as gh;
use gpui::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, Context, Entity, Hsla, PathPromptOptions,
    SharedString, Window,
};

use crate::attach_flow::{attach_notice, attach_paths};
use crate::date::{format_date, format_iso_date, parse_date};
use crate::external::{open_attachment, reveal, OpenOutcome};
use crate::issue_sync::{load_token, remote_for, IssueSync};
use crate::markdown::{self, Block};
use crate::markdown_view;
use crate::preview_select;
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;
use crate::ui::{
    back_button, button, chip, empty_state, icon, icon_button, icon_chip, notice, section_label,
    segment, segmented, status_style, tab, tab_bar, ButtonKind, Icon,
};

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;
pub type OpenNoteHandler = Box<dyn Fn(NoteId, SharedString, &mut Window, &mut App)>;
/// Opens a note in the editor, with the cursor at a byte offset when given.
pub type EditNoteHandler = Box<dyn Fn(NoteId, SharedString, Option<usize>, &mut Window, &mut App)>;

/// The parsed body note with what a click needs to find its place in the
/// source; kept until the note changes.
struct BodyCache {
    note: NoteId,
    updated: i64,
    blocks: Vec<Block>,
    source: String,
    /// Where each block starts in `source`.
    starts: Vec<Option<usize>>,
}
/// Opens a task's page; the flag asks for its Subtasks tab.
pub type OpenTaskHandler = Box<dyn Fn(TaskId, bool, &mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Description,
    Subtasks,
    Attachments,
    Commits,
    /// The issue thread; only for tasks that are GitHub issues.
    Comments,
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
            "description" | "notes" => Tab::Description,
            "subtasks" => Tab::Subtasks,
            "comments" => Tab::Comments,
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
    /// "New subtask…" input on the Subtasks tab; Enter adds it.
    new_subtask_input: Entity<TextInput>,
    /// Pulls and pushes this task's GitHub issue; also says what went wrong.
    issue_sync: Entity<IssueSync>,
    /// The parsed body note, kept until the note changes.
    body_cache: Option<BodyCache>,
    /// Text selection of the body preview (each text registers with it).
    preview_select: preview_select::Shared,
    /// Foldable alerts of the description that the reader has flipped.
    alert_folds: markdown_view::AlertFolds,
    /// Focus scope of the description: Ctrl+C / Ctrl+A / Ctrl+F act on it.
    preview_focus: gpui::FocusHandle,
    /// Find bar of the description (Ctrl+F).
    find_input: Entity<TextInput>,
    find_open: bool,
    /// Scroll of the page body, to bring the current match into view.
    body_scroll: gpui::ScrollHandle,
    /// Index of the code block whose copy button was just pressed.
    copied: Option<usize>,
    /// The issue thread, read when the Comments tab opens.
    comments: Option<Vec<gh::Comment>>,
    comments_busy: bool,
    comments_error: Option<String>,
    comment_input: Entity<TextInput>,
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
    /// Opens a note straight in Edit mode (the description's Edit button and
    /// double-click); `on_open_note` opens it for reading.
    pub on_edit_note: Option<EditNoteHandler>,
    /// Opens another task's page (a subtask, or the parent on Back).
    pub on_open_task: Option<OpenTaskHandler>,
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
        issue_sync: Entity<IssueSync>,
    ) -> Self {
        let new_subtask_input = cx.new(|cx| TextInput::new(cx, "New subtask…", theme));
        let me = cx.entity();
        new_subtask_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |title, _window, cx| {
                let title = title.trim().to_string();
                if title.is_empty() {
                    return;
                }
                me.update(cx, |this, cx| this.create_subtask(&title, cx));
            }));
        });
        let comment_input = cx.new(|cx| TextInput::new(cx, "Write a comment", theme));
        comment_input.update(cx, |input, _| {
            input.multiline = true;
            input.min_rows = 3;
        });
        cx.observe(&comment_input, |_, _, cx| cx.notify()).detach();
        // Sync progress and failures show on this page too.
        cx.observe(&issue_sync, |_, _, cx| cx.notify()).detach();

        let tag_input = cx.new(|cx| TextInput::new(cx, "Add tag…", theme));
        let me_for_tags = cx.entity();
        tag_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |tag, _window, cx| {
                let tag = tag.to_string();
                me_for_tags.update(cx, |this, cx| this.add_tag(&tag, cx));
            }));
        });

        let find_input = cx.new(|cx| TextInput::new(cx, "Find in description", theme));
        let weak = cx.weak_entity();
        find_input.update(cx, |input, _| {
            input.keep_on_confirm = true;
            let next = weak.clone();
            input.on_confirm = Some(Box::new(move |_, window, cx| {
                let next = next.clone();
                window.defer(cx, move |_, cx| {
                    next.update(cx, |this, cx| this.find_step(1, cx)).ok();
                });
            }));
            let close = weak.clone();
            input.on_cancel = Some(Box::new(move |window, cx| {
                let close = close.clone();
                window.defer(cx, move |window, cx| {
                    close
                        .update(cx, |this, cx| this.close_find(window, cx))
                        .ok();
                });
            }));
        });
        cx.observe(&find_input, |_, _, cx| cx.notify()).detach();

        // Links/tasks/notes change from elsewhere (commit browser, deletes).
        cx.observe(&state, |this, _, cx| {
            this.refresh_commits(cx);
            cx.notify();
        })
        .detach();

        // Opens on the description; a task with subtasks and nothing written
        // opens on its subtasks instead.
        let tab = {
            let s = state.read(cx);
            let has_text = s
                .body_note(task_id)
                .and_then(|id| s.read_note_content(id))
                .is_some_and(|t| !t.trim().is_empty());
            let has_subtasks = s.tasks.iter().any(|t| t.parent_id == Some(task_id));
            if !has_text && has_subtasks {
                Tab::Subtasks
            } else {
                Tab::Description
            }
        };

        Self {
            state,
            theme,
            task_id,
            tab,
            new_subtask_input,
            issue_sync,
            body_cache: None,
            preview_select: Default::default(),
            alert_folds: Default::default(),
            preview_focus: cx.focus_handle(),
            find_input,
            find_open: false,
            body_scroll: gpui::ScrollHandle::new(),
            copied: None,
            comments: None,
            comments_busy: false,
            comments_error: None,
            comment_input,
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
            on_edit_note: None,
            on_open_task: None,
            on_open_history: None,
        }
    }

    /// Ctrl+F in the description: shows the find bar (seeded with a one-line
    /// selection) and focuses its query box.
    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        let selected = self.preview_select.borrow().selected_text();
        if let Some(text) = selected.filter(|t| !t.contains('\n') && t.len() <= 120) {
            self.find_input.update(cx, |input, cx| {
                input.set_text_selected(text);
                cx.notify();
            });
        }
        let focus = self.find_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes the find bar and hands focus back to the description.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        self.preview_select.borrow_mut().set_query("");
        window.focus(&self.preview_focus, cx);
        cx.notify();
    }

    /// Steps to the next (`+1`) or previous (`-1`) match.
    fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.preview_select.borrow_mut().step_hit(delta);
        cx.notify();
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

    /// Adds a subtask under this task. It is local until published (or
    /// until this task is an issue and the subtask is published too).
    fn create_subtask(&mut self, title: &str, cx: &mut Context<Self>) {
        let parent = self.task_id;
        let made = self.state.update(cx, |state, cx| {
            let id = state.create_subtask(parent, title);
            cx.notify();
            id
        });
        if made.is_none() {
            self.notice = Some(("Couldn't create the subtask".into(), true));
        }
        cx.notify();
    }

    /// Opens the task's body note in the note page (made first if the task
    /// has none yet).
    fn open_body_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_body_note_at(None, window, cx);
    }

    /// Like [`Self::open_body_note`], with the editor's cursor at `offset` (a
    /// byte offset into the description) when given.
    fn open_body_note_at(
        &mut self,
        offset: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = self.task_id;
        let note = self.state.update(cx, |state, cx| {
            let id = state.ensure_body_note(task);
            cx.notify();
            id.and_then(|id| {
                state
                    .notes
                    .iter()
                    .find(|n| n.id == id)
                    .map(|n| (id, n.title.clone()))
            })
        });
        match note {
            Some((id, title)) => {
                if let Some(open) = &self.on_edit_note {
                    open(id, title.into(), offset, window, cx);
                }
            }
            None => self.notice = Some(("Couldn't open the description".into(), true)),
        }
        cx.notify();
    }

    /// Ticks or unticks the checklist marker at `range` in the description (a
    /// click on a preview checkbox). Skipped if the text no longer matches what
    /// the preview saw.
    fn toggle_body_task(&mut self, range: &std::ops::Range<usize>, cx: &mut Context<Self>) {
        let task = self.task_id;
        self.state.update(cx, |state, cx| {
            let Some(id) = state.body_note(task) else {
                return;
            };
            let Some(content) = state.read_note_content(id) else {
                return;
            };
            let Some((replace, text)) = markdown::toggle_marker(&content, range) else {
                return;
            };
            let mut updated = content;
            updated.replace_range(replace, text);
            state.write_note_content(id, &updated);
            cx.notify();
        });
        cx.notify();
    }

    /// A click on a link in the description: a `[[Title]]` reference opens the
    /// note or task of this project it names; anything else is a web or mail
    /// link (the target is untrusted, so `open_url` filters it).
    fn follow_link(&mut self, href: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(title) = markdown::ref_target(href) {
            let target = {
                let state = self.state.read(cx);
                crate::refs::resolve(title, &state.notes, &state.tasks)
            };
            match target {
                Some(crate::refs::RefTarget::Note(id)) => {
                    if let Some(open) = &self.on_open_note {
                        open(id, SharedString::from(title.trim().to_string()), window, cx);
                    }
                }
                Some(crate::refs::RefTarget::Task(id)) => self.open_task(id, window, cx),
                None => {
                    self.notice =
                        Some((format!("No note or task named \"{}\"", title.trim()), true));
                    cx.notify();
                }
            }
            return;
        }
        let _ = crate::external::open_url(href);
    }

    /// Opens another task's page (a subtask, or the parent).
    fn open_task(&mut self, id: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &self.on_open_task {
            open(id, false, window, cx);
        }
    }

    /// Opens this task's issue on github.com.
    fn open_issue_in_browser(&mut self, cx: &mut Context<Self>) {
        let target = {
            let state = self.state.read(cx);
            state.issue_link(self.task_id).and_then(|link| {
                let repo = state.repos.iter().find(|r| r.id == link.repo_id)?;
                Some((repo.path.clone(), link.number))
            })
        };
        let Some((path, number)) = target else { return };
        cx.spawn(async move |this, cx| {
            let url = cx
                .background_executor()
                .spawn(async move {
                    remote_for(&path).map(|r| {
                        format!("https://github.com/{}/{}/issues/{number}", r.owner, r.name)
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let result = match url {
                    Some(url) => crate::external::open_url(&url),
                    None => Err("The repo has no GitHub remote.".into()),
                };
                if let Err(message) = result {
                    this.notice = Some((message, true));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads the issue's comments in the background.
    fn load_comments(&mut self, cx: &mut Context<Self>) {
        let target = {
            let state = self.state.read(cx);
            state.issue_link(self.task_id).and_then(|link| {
                let repo = state.repos.iter().find(|r| r.id == link.repo_id)?;
                Some((repo.path.clone(), link.number))
            })
        };
        let Some((path, number)) = target else {
            self.comments = None;
            return;
        };
        if self.comments_busy {
            return;
        }
        self.comments_busy = true;
        self.comments_error = None;
        cx.notify();
        let task = self.task_id;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let token = load_token().ok_or("Sign in to GitHub in Settings first.")?;
                    let remote = remote_for(&path).ok_or("The repo has no GitHub remote.")?;
                    gh::list_comments(&token, &remote, number).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.comments_busy = false;
                if this.task_id == task {
                    match result {
                        Ok(list) => this.comments = Some(list),
                        Err(message) => this.comments_error = Some(message),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Posts the written comment to the issue.
    fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.comment_input.read(cx).content.trim().to_string();
        if text.is_empty() || self.comments_busy {
            return;
        }
        let target = {
            let state = self.state.read(cx);
            state.issue_link(self.task_id).and_then(|link| {
                let repo = state.repos.iter().find(|r| r.id == link.repo_id)?;
                Some((repo.path.clone(), link.number))
            })
        };
        let Some((path, number)) = target else { return };
        self.comments_busy = true;
        self.comments_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let token = load_token().ok_or("Sign in to GitHub in Settings first.")?;
                    let remote = remote_for(&path).ok_or("The repo has no GitHub remote.")?;
                    gh::add_comment(&token, &remote, number, &text).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.comments_busy = false;
                match result {
                    Ok(comment) => {
                        this.comments.get_or_insert_with(Vec::new).push(comment);
                        this.comment_input
                            .update(cx, |input, cx| input.reset(window, cx));
                    }
                    Err(message) => this.comments_error = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
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

    /// The task's body: its first note, rendered. Editing opens the note
    /// page. Older tasks may carry further notes; they are listed below.
    fn description_tab(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme;
        let task_id = self.task_id;
        let (body, extras) = {
            let state = self.state.read(cx);
            let body = state.body_note(task_id).and_then(|id| {
                state
                    .notes
                    .iter()
                    .find(|n| n.id == id)
                    .map(|n| (id, n.updated_at))
            });
            (body, state.extra_notes(task_id))
        };
        let mut out: Vec<AnyElement> = Vec::new();
        match body {
            None => out.push(
                empty_state(
                    &theme,
                    Icon::FileText,
                    "No description yet",
                    "Describe the task in markdown: context, decisions, scripts, SQL.",
                    Some(
                        button(
                            &theme,
                            "task-add-description",
                            ButtonKind::Primary,
                            Some(Icon::Pencil),
                            "Add description",
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_body_note(window, cx)),
                        )
                        .into_any_element(),
                    ),
                )
                .into_any_element(),
            ),
            Some((id, updated)) => {
                let stale = self
                    .body_cache
                    .as_ref()
                    .is_none_or(|c| (c.note, c.updated) != (id, updated));
                if stale {
                    let content = self
                        .state
                        .read(cx)
                        .read_note_content(id)
                        .unwrap_or_default();
                    let (blocks, starts) = markdown::parse_mapped(&content);
                    self.body_cache = Some(BodyCache {
                        note: id,
                        updated,
                        blocks,
                        source: content,
                        starts,
                    });
                    // The old selection pointed into text that is gone.
                    self.preview_select.borrow_mut().clear();
                }
                let query = if self.find_open {
                    self.find_input.read(cx).content.to_string()
                } else {
                    String::new()
                };
                self.preview_select.borrow_mut().set_query(&query);
                let blocks: &[Block] = self.body_cache.as_ref().map_or(&[], |c| &c.blocks);
                // How `[[Title]]` and `#4` are drawn depends on the project's
                // notes and tasks; snapshot them only for a body that has any.
                let ref_lookup: Option<markdown_view::RefLookup> = self
                    .body_cache
                    .as_ref()
                    .filter(|c| crate::refs::mentions_refs(&c.source))
                    .map(|_| {
                        let state = self.state.read(cx);
                        crate::refs::lookup(&state.notes, &state.tasks)
                    });
                out.push(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(section_label(&theme, "Description"))
                        .child(
                            button(
                                &theme,
                                "task-edit-description",
                                ButtonKind::Secondary,
                                Some(Icon::Pencil),
                                "Edit",
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_body_note(window, cx)),
                            ),
                        )
                        .into_any_element(),
                );
                if blocks.is_empty() {
                    out.push(
                        div()
                            .t_small()
                            .text_color(theme.text_muted)
                            .child(
                                "The description is empty. Double-click or press Edit to write it.",
                            )
                            .into_any_element(),
                    );
                }
                let me = cx.entity();
                let edit_me = me.clone();
                let toggle_me = me.clone();
                let link_me = me.clone();
                let hook = markdown_view::CopyHook {
                    select: self.preview_select.clone(),
                    on_edit: Some(std::rc::Rc::new(move |block, at, window, cx| {
                        edit_me.update(cx, |this, cx| {
                            // Cursor on the clicked letter, not just the block.
                            let clicked = this.preview_select.borrow().text_at(at);
                            let offset = this.body_cache.as_ref().and_then(|c| {
                                markdown::click_offset(&c.source, &c.starts, block, clicked)
                            });
                            this.open_body_note_at(offset, window, cx)
                        });
                    })),
                    copied: self.copied,
                    on_copy: std::rc::Rc::new(move |index, _text, _window, cx| {
                        me.update(cx, |this, cx| {
                            this.copied = Some(index);
                            cx.notify();
                            cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(1600))
                                    .await;
                                let _ = this.update(cx, |this, cx| {
                                    if this.copied == Some(index) {
                                        this.copied = None;
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                        });
                    }),
                    // Ticking a box rewrites its marker in the body note, which
                    // refreshes the cached preview through the note's timestamp.
                    on_toggle: std::rc::Rc::new(move |range, _window, cx| {
                        toggle_me.update(cx, |this, cx| this.toggle_body_task(&range, cx));
                    }),
                    on_link: std::rc::Rc::new(move |href, window, cx| {
                        link_me.update(cx, |this, cx| this.follow_link(&href, window, cx));
                    }),
                    refs: ref_lookup,
                    folds: Some(self.alert_folds.clone()),
                };
                let select = self.preview_select.clone();
                let (down_select, move_select, up_select, out_select, all_select) = (
                    select.clone(),
                    select.clone(),
                    select.clone(),
                    select.clone(),
                    select.clone(),
                );
                let (reveal_select, reveal_scroll) = (select.clone(), self.body_scroll.clone());
                let focus = self.preview_focus.clone();
                let down_focus = focus.clone();
                let rendered = markdown_view::render_blocks(blocks, &theme, &hook);
                if self.find_open {
                    let (total, current, searching) = {
                        let s = select.borrow();
                        (s.hit_total(), s.current_hit(), !s.query().is_empty())
                    };
                    let status = match (searching, total) {
                        (false, _) => String::new(),
                        (true, 0) => "No results".to_string(),
                        (true, n) => format!("{} of {n}", current + 1),
                    };
                    out.push(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(div().w(px(240.0)).child(self.find_input.clone()))
                            .child(
                                div()
                                    .min_w(px(64.0))
                                    .t_small()
                                    .text_color(theme.text_muted)
                                    .child(status),
                            )
                            .child(
                                icon_button(
                                    &theme,
                                    Icon::ChevronUp,
                                    "tfind-prev",
                                    theme.text_primary,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.find_step(-1, cx))),
                            )
                            .child(
                                icon_button(
                                    &theme,
                                    Icon::ChevronDown,
                                    "tfind-next",
                                    theme.text_primary,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.find_step(1, cx))),
                            )
                            .child(
                                icon_button(&theme, Icon::X, "tfind-close", theme.text_primary)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_find(window, cx)
                                    })),
                            )
                            .into_any_element(),
                    );
                }
                out.push(
                    div()
                        .id("task-description")
                        .key_context("NotePreview")
                        .track_focus(&focus)
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(theme.space_3)
                        .on_action(
                            cx.listener(move |_, _: &preview_select::PreviewCopy, _, cx| {
                                if let Some(text) = select.borrow().selected_text() {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                }
                            }),
                        )
                        .on_action(cx.listener(
                            move |_, _: &preview_select::PreviewSelectAll, _, cx| {
                                all_select.borrow_mut().select_all();
                                cx.notify();
                            },
                        ))
                        .on_action(cx.listener(
                            |this, _: &preview_select::PreviewFind, window, cx| {
                                this.open_find(window, cx)
                            },
                        ))
                        .on_action(cx.listener(
                            |this, _: &preview_select::PreviewFindNext, _, cx| {
                                this.find_step(1, cx)
                            },
                        ))
                        .on_action(cx.listener(
                            |this, _: &preview_select::PreviewFindPrev, _, cx| {
                                this.find_step(-1, cx)
                            },
                        ))
                        // Drag to select text; a double-click still edits, so
                        // only a first press begins a selection.
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |_, event: &gpui::MouseDownEvent, window, cx| {
                                window.focus(&down_focus, cx);
                                if event.click_count == 1 {
                                    down_select.borrow_mut().start(event.position);
                                }
                                cx.notify();
                            }),
                        )
                        .on_mouse_move(cx.listener(
                            move |_, event: &gpui::MouseMoveEvent, _, cx| {
                                if event.pressed_button == Some(gpui::MouseButton::Left)
                                    && move_select.borrow().is_dragging()
                                    && move_select.borrow_mut().drag(event.position)
                                {
                                    cx.notify();
                                }
                            },
                        ))
                        .on_mouse_up(
                            gpui::MouseButton::Left,
                            cx.listener(move |_, _, _, cx| {
                                up_select.borrow_mut().finish();
                                cx.notify();
                            }),
                        )
                        .on_mouse_up_out(
                            gpui::MouseButton::Left,
                            cx.listener(move |_, _, _, cx| {
                                out_select.borrow_mut().finish();
                                cx.notify();
                            }),
                        )
                        .children(rendered)
                        // Last, so the texts above are measured when it runs:
                        // scrolls the current find match into view.
                        .child(
                            gpui::canvas(
                                move |_, window, _| {
                                    let mut select = reveal_select.borrow_mut();
                                    if !select.take_reveal() {
                                        return;
                                    }
                                    let Some((at, line_height)) = select.current_hit_position()
                                    else {
                                        return;
                                    };
                                    let viewport = reveal_scroll.bounds();
                                    let margin = line_height * 2.0;
                                    let mut offset = reveal_scroll.offset();
                                    if at.y < viewport.top() + margin {
                                        offset.y += viewport.top() + margin - at.y;
                                    } else if at.y + line_height > viewport.bottom() - margin {
                                        offset.y +=
                                            viewport.bottom() - margin - (at.y + line_height);
                                    }
                                    offset.y =
                                        offset.y.min(px(0.0)).max(-reveal_scroll.max_offset().y);
                                    if offset != reveal_scroll.offset() {
                                        reveal_scroll.set_offset(offset);
                                        window.refresh();
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute(),
                        )
                        .into_any_element(),
                );
            }
        }
        if !extras.is_empty() {
            out.push(self.section_header("More notes".into()));
            out.extend(extras.into_iter().map(|note| {
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
                            .flex_1()
                            .min_w(px(0.0))
                            .t_body()
                            .text_color(theme.text_primary)
                            .truncate()
                            .child(note.title.clone()),
                    )
                    .child(
                        div()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(format!("updated {}", format_date(note.updated_at))),
                    )
                    .into_any_element()
            }));
        }
        out
    }

    /// Direct subtasks, each a task with its own page. On GitHub they are
    /// sub-issues once published.
    fn subtasks_tab(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme;
        let task_id = self.task_id;
        let mut children: Vec<(Task, usize, Option<u64>)> = {
            let state = self.state.read(cx);
            state
                .tasks
                .iter()
                .filter(|t| t.parent_id == Some(task_id))
                .map(|t| {
                    let grand = state
                        .tasks
                        .iter()
                        .filter(|g| g.parent_id == Some(t.id))
                        .count();
                    (t.clone(), grand, state.issue_link(t.id).map(|l| l.number))
                })
                .collect()
        };
        // Most recently updated first.
        children.sort_by_key(|(t, _, _)| std::cmp::Reverse(t.updated_at));
        let mut out: Vec<AnyElement> = vec![
            div()
                .child(self.new_subtask_input.clone())
                .into_any_element(),
            div()
                .px(theme.space_1)
                .t_small()
                .text_color(theme.text_muted)
                .child("Subtasks are tasks: each has its own description, files and commits.")
                .into_any_element(),
        ];
        if children.is_empty() {
            out.push(self.empty(
                Icon::ListChecks,
                "No subtasks",
                "Break the task down: type a title above and press Enter.",
            ));
        }
        let rows = children.into_iter().map(|(child, grand, number)| {
            let id = child.id;
            let color = status_style(&theme, child.status).1;
            let updated = format_date(child.updated_at);
            let subtitle = match grand {
                0 => format!("updated {updated}"),
                1 => format!("updated {updated} · 1 subtask"),
                n => format!("updated {updated} · {n} subtasks"),
            };
            // A flat row like the project page's: icon, title over an
            // "updated" line, and the status as a tag at the far right.
            self.row()
                .id(("subtask", id.0 as u64))
                .cursor_pointer()
                .hover(move |s| s.bg(theme.surface_hover))
                .on_click(cx.listener(move |this, _, window, cx| this.open_task(id, window, cx)))
                .child(icon(Icon::FileText, 16.0, theme.text_muted))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .t_body()
                                .text_color(if child.status == TaskStatus::Done {
                                    theme.text_secondary
                                } else {
                                    theme.text_primary
                                })
                                .truncate()
                                .child(child.title.clone()),
                        )
                        .child(
                            div()
                                .t_small()
                                .t_code()
                                .text_color(theme.text_muted)
                                .child(subtitle),
                        ),
                )
                .children(number.map(|n| chip(&theme, format!("#{n}"), theme.text_secondary)))
                .child(
                    div()
                        .id(("subtask-status", id.0 as u64))
                        .flex_none()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.cycle_task_status(id);
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }))
                        .child(chip(&theme, status_label(child.status), color)),
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

    /// The issue's comment thread (GitHub only).
    fn comments_tab(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme;
        if self.comments.is_none() && !self.comments_busy && self.comments_error.is_none() {
            self.load_comments(cx);
        }
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(message) = self.comments_error.clone() {
            out.push(notice(&theme, message, true).into_any_element());
        }
        match &self.comments {
            None => out.push(
                div()
                    .t_small()
                    .text_color(theme.text_muted)
                    .child(if self.comments_busy {
                        "Loading comments…"
                    } else {
                        "Comments could not be loaded."
                    })
                    .into_any_element(),
            ),
            Some(list) if list.is_empty() => out.push(
                div()
                    .t_small()
                    .text_color(theme.text_muted)
                    .child("No comments yet.")
                    .into_any_element(),
            ),
            Some(list) => {
                for comment in list {
                    out.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space_1)
                            .p(theme.space_3)
                            .rounded(theme.radius_md)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .bg(theme.surface_sunken)
                            .child(div().t_small().t_code().text_color(theme.text_muted).child(
                                format!(
                                    "{} · {}",
                                    comment.author,
                                    comment.created_at.get(..10).unwrap_or(&comment.created_at)
                                ),
                            ))
                            .child(
                                div()
                                    .t_body()
                                    .text_color(theme.text_secondary)
                                    .child(comment.body.clone()),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        let has_text = !self.comment_input.read(cx).content.trim().is_empty();
        out.push(
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .child(self.comment_input.clone())
                .child(
                    div().flex().justify_end().child(
                        button(
                            &theme,
                            "task-post-comment",
                            ButtonKind::Primary,
                            Some(Icon::Upload),
                            "Comment",
                        )
                        .when(!has_text || self.comments_busy, |b| b.opacity(0.5))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.post_comment(window, cx);
                        })),
                    ),
                )
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
        NoteKind::Todo => Icon::ListChecks,
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

        let (task, trail, subtasks, attachments, links, issue, github_ready) = {
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
                state
                    .tasks
                    .iter()
                    .filter(|t| t.parent_id == Some(self.task_id))
                    .count(),
                state.attachments_for_task(self.task_id),
                links,
                state.issue_link(self.task_id).cloned(),
                !state.repos.is_empty(),
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

        let counts = (subtasks, attachments.len(), links.len());
        // The Comments tab exists only for issues; fall back if it vanished.
        if self.tab == Tab::Comments && issue.is_none() {
            self.tab = Tab::Description;
        }
        let body = match self.tab {
            Tab::Description => self.description_tab(cx),
            Tab::Subtasks => self.subtasks_tab(cx),
            Tab::Comments => self.comments_tab(cx),
            Tab::Attachments => self.attachments_tab(attachments, cx),
            Tab::Commits => self.commits_tab(links, cx),
        };
        let (status_glyph, status_color) = status_style(&theme, task.status);
        let parent_id = task.parent_id;
        let github_on = self.issue_sync.read(cx).enabled(cx);
        let (sync_error, sync_notice, sync_busy) = {
            let sync = self.issue_sync.read(cx);
            (sync.error.clone(), sync.notice.clone(), sync.busy > 0)
        };
        let issue_number = issue.as_ref().map(|l| l.number);

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
                                move |this, _, window, cx| {
                                    // A subtask goes back to its parent, a top-level task
                                    // back to the project page.
                                    match (parent_id, &this.on_open_task) {
                                        (Some(parent), Some(open)) => {
                                            open(parent, true, window, cx)
                                        }
                                        _ => {
                                            if let Some(close) = &this.on_close {
                                                close(window, cx);
                                            }
                                        }
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
                            .child(div().flex_none().child(self.details_row(&task, cx)))
                            .child(div().flex_none().child(self.tags_row(&task, cx)))
                            .children(match issue_number {
                                Some(number) => Some(
                                    div()
                                        .id("task-issue-link")
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .gap(theme.space_2)
                                        .h(px(28.0))
                                        .px(theme.space_3)
                                        .rounded(theme.radius_sm)
                                        .border_1()
                                        .border_color(theme.border_light)
                                        .text_color(theme.text_secondary)
                                        .t_small()
                                        .t_code()
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(theme.surface_hover))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.open_issue_in_browser(cx);
                                        }))
                                        .child(icon(Icon::CircleDot, 14.0, theme.text_secondary))
                                        .child(format!("#{number}"))
                                        .into_any_element(),
                                ),
                                None if github_on && github_ready => Some(
                                    button(
                                        &theme,
                                        "task-publish",
                                        ButtonKind::Secondary,
                                        Some(Icon::Upload),
                                        if sync_busy {
                                            "Publishing…"
                                        } else {
                                            "Publish to GitHub"
                                        },
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let task = this.task_id;
                                        this.issue_sync
                                            .update(cx, |sync, cx| sync.publish(task, cx));
                                    }))
                                    .into_any_element(),
                                ),
                                None => None,
                            })
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
                    .child(
                        tab_bar(&theme)
                            .child(
                                tab(
                                    &theme,
                                    "tab-description",
                                    "Description",
                                    None,
                                    self.tab == Tab::Description,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.tab = Tab::Description;
                                        this.notice = None;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                tab(
                                    &theme,
                                    "tab-subtasks",
                                    "Subtasks",
                                    Some(counts.0),
                                    self.tab == Tab::Subtasks,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.tab = Tab::Subtasks;
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
                            )
                            .children(issue_number.map(|_| {
                                tab(
                                    &theme,
                                    "tab-comments",
                                    "Comments",
                                    None,
                                    self.tab == Tab::Comments,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.tab = Tab::Comments;
                                        this.notice = None;
                                        this.load_comments(cx);
                                        cx.notify();
                                    },
                                ))
                            })),
                    ),
            )
            .when_some(
                sync_error
                    .map(|m| (m, true))
                    .or(sync_notice.map(|m| (m, false))),
                |d, (message, is_error)| {
                    d.child(
                        div()
                            .px(theme.space_6)
                            .pt(theme.space_3)
                            .child(notice(&theme, message, is_error)),
                    )
                },
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
                    .track_scroll(&self.body_scroll)
                    .children(body),
            )
            .into_any_element()
    }
}
