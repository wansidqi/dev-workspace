//! Commit history browser (CLAUDE.md "Git integration"): a GitHub-Desktop
//! style list of commits merged across every repo linked to the project,
//! with live search, repo filter, an "unassigned" filter, multi-select
//! (click / Ctrl-click / Shift-click range) and a side panel that maps the
//! selection to any task in the project's tree with a status label and note.
//!
//! Git history is read on a background task one page at a time and the list
//! is virtualized, so thousands of commits stay cheap (CLAUDE.md GPUI and
//! performance rules).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use devspace_core::{CommitLink, CommitLinkStatus, Repo, RepoId, TaskId};
use devspace_git::{HistoryCursor, RepoCommit};
use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, App, Context, Entity, Hsla, SharedString,
    UniformListScrollHandle, Window,
};

use crate::date::format_date;
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;

/// Commits fetched per background page.
const PAGE_SIZE: usize = 200;
/// Fetch the next page when the viewport gets this close to the end.
const PREFETCH_ROWS: usize = 40;
const ROW_HEIGHT: f32 = 56.0;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
const SIDE_PANEL_WIDTH: f32 = 360.0;

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// A commit's identity: a hash is only unique within one repo.
type CommitKey = (RepoId, String);

pub struct CommitBrowser {
    state: Entity<AppState>,
    theme: GlassTheme,
    search_input: Entity<TextInput>,
    note_input: Entity<TextInput>,
    scroll: UniformListScrollHandle,
    pub on_close: Option<CloseHandler>,

    commits: Vec<RepoCommit>,
    /// Indices into `commits` that pass the client-side filters, in order.
    visible: Vec<usize>,
    cursor: Option<HistoryCursor>,
    has_more: bool,
    loading: bool,
    /// Bumped on every reload so a slow page for an old query is ignored.
    generation: u64,
    unreadable: Vec<String>,

    /// Text currently typed, and the text the loaded results are for.
    typed_query: String,
    applied_query: String,
    search_epoch: u64,
    repo_filter: Option<RepoId>,
    unassigned_only: bool,

    selected: HashSet<CommitKey>,
    /// Row (index into `visible`) a Shift-click range extends from.
    anchor: Option<usize>,
    new_status: CommitLinkStatus,
    /// Result of the last map action: (message, is_error).
    notice: Option<(String, bool)>,
}

fn status_label(status: CommitLinkStatus) -> &'static str {
    match status {
        CommitLinkStatus::Linked => "Linked",
        CommitLinkStatus::NeedsMigration => "Needs migration",
        CommitLinkStatus::CherryPicked => "Cherry-picked",
        CommitLinkStatus::Deployed => "Deployed",
        CommitLinkStatus::Missing => "Missing",
    }
}

fn status_color(theme: &GlassTheme, status: CommitLinkStatus) -> Hsla {
    match status {
        CommitLinkStatus::Linked => theme.accent,
        CommitLinkStatus::NeedsMigration => theme.warning,
        CommitLinkStatus::CherryPicked => theme.accent,
        CommitLinkStatus::Deployed => theme.success,
        CommitLinkStatus::Missing => theme.danger,
    }
}

const SELECTABLE_STATUSES: [CommitLinkStatus; 4] = [
    CommitLinkStatus::Linked,
    CommitLinkStatus::NeedsMigration,
    CommitLinkStatus::CherryPicked,
    CommitLinkStatus::Deployed,
];

impl CommitBrowser {
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        let search_input =
            cx.new(|cx| TextInput::new(cx, "Search commits by title, author or hash…", theme));
        let note_input = cx.new(|cx| TextInput::new(cx, "Note (optional)…", theme));

        // Live search: the input notifies on every edit; compare and debounce.
        cx.observe(&search_input, |this, input, cx| {
            let typed = input.read(cx).content.to_string();
            if typed == this.typed_query {
                return;
            }
            this.typed_query = typed;
            this.search_epoch += 1;
            let epoch = this.search_epoch;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                let _ = this.update(cx, |this, cx| {
                    if this.search_epoch == epoch && this.applied_query != this.typed_query {
                        this.applied_query = this.typed_query.clone();
                        this.reload(cx);
                    }
                });
            })
            .detach();
        })
        .detach();

        // Links/tasks change from outside (mapping, unlinking): re-filter.
        cx.observe(&state, |this, _, cx| {
            this.refilter(cx);
            cx.notify();
        })
        .detach();

        let mut browser = Self {
            state,
            theme,
            search_input,
            note_input,
            scroll: UniformListScrollHandle::new(),
            on_close: None,
            commits: Vec::new(),
            visible: Vec::new(),
            cursor: None,
            has_more: true,
            loading: false,
            generation: 0,
            unreadable: Vec::new(),
            typed_query: String::new(),
            applied_query: String::new(),
            search_epoch: 0,
            repo_filter: None,
            unassigned_only: false,
            selected: HashSet::new(),
            anchor: None,
            new_status: CommitLinkStatus::Linked,
            notice: None,
        };
        browser.load_more(cx);
        browser
    }

    /// Starts over for a new query/filter: drops results and loads page one.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.commits.clear();
        self.visible.clear();
        self.cursor = None;
        self.has_more = true;
        self.loading = false;
        self.selected.clear();
        self.anchor = None;
        self.unreadable.clear();
        self.load_more(cx);
        cx.notify();
    }

    fn repos_in_scope(&self, cx: &App) -> Vec<Repo> {
        self.state
            .read(cx)
            .repos
            .iter()
            .filter(|r| self.repo_filter.is_none_or(|id| id == r.id))
            .cloned()
            .collect()
    }

    /// Fetches the next page on a background task.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.has_more {
            return;
        }
        self.loading = true;
        let generation = self.generation;
        let repos = self.repos_in_scope(cx);
        let cursor = self.cursor.clone();
        let query = self.applied_query.clone();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move {
                    devspace_git::search_history(&repos, PAGE_SIZE, cursor.as_ref(), &query)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                this.has_more = page.next.is_some();
                this.cursor = page.next;
                let names: Vec<String> = {
                    let state = this.state.read(cx);
                    page.unreadable
                        .iter()
                        .filter_map(|id| state.repos.iter().find(|r| r.id == *id))
                        .map(|r| r.name.clone())
                        .collect()
                };
                for name in names {
                    if !this.unreadable.contains(&name) {
                        this.unreadable.push(name);
                    }
                }
                this.commits.extend(page.commits);
                this.refilter(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Links grouped by commit, for badges and the unassigned filter.
    fn links_by_commit(links: &[CommitLink]) -> HashMap<CommitKey, Vec<&CommitLink>> {
        let mut map: HashMap<CommitKey, Vec<&CommitLink>> = HashMap::new();
        for link in links {
            map.entry((link.repo_id, link.hash.clone()))
                .or_default()
                .push(link);
        }
        map
    }

    fn refilter(&mut self, cx: &App) {
        if !self.unassigned_only {
            self.visible = (0..self.commits.len()).collect();
            return;
        }
        let state = self.state.read(cx);
        let linked: HashSet<CommitKey> = state
            .commit_links
            .iter()
            .map(|l| (l.repo_id, l.hash.clone()))
            .collect();
        self.visible = self
            .commits
            .iter()
            .enumerate()
            .filter(|(_, c)| !linked.contains(&(c.repo_id, c.commit.hash.clone())))
            .map(|(i, _)| i)
            .collect();
    }

    fn key_at(&self, visible_ix: usize) -> Option<CommitKey> {
        let commit = self.commits.get(*self.visible.get(visible_ix)?)?;
        Some((commit.repo_id, commit.commit.hash.clone()))
    }

    /// Click semantics like a file manager: plain click selects one,
    /// Ctrl/Cmd toggles, Shift selects the range from the anchor.
    fn click_row(&mut self, visible_ix: usize, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(key) = self.key_at(visible_ix) else {
            return;
        };
        self.notice = None;
        if modifiers.shift && self.anchor.is_some() {
            let anchor = self.anchor.unwrap_or(visible_ix);
            let (lo, hi) = (anchor.min(visible_ix), anchor.max(visible_ix));
            self.selected = (lo..=hi).filter_map(|i| self.key_at(i)).collect();
        } else if modifiers.control || modifiers.platform {
            if !self.selected.remove(&key) {
                self.selected.insert(key);
            }
            self.anchor = Some(visible_ix);
        } else {
            self.selected.clear();
            self.selected.insert(key);
            self.anchor = Some(visible_ix);
        }
        cx.notify();
    }

    fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        self.selected = (0..self.visible.len())
            .filter_map(|i| self.key_at(i))
            .collect();
        cx.notify();
    }

    /// Selected commits, newest first, as they appear in the list.
    fn selected_in_order(&self) -> Vec<CommitKey> {
        (0..self.visible.len())
            .filter_map(|i| self.key_at(i))
            .filter(|k| self.selected.contains(k))
            .collect()
    }

    fn map_selection_to(
        &mut self,
        task: TaskId,
        task_title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commits = self.selected_in_order();
        if commits.is_empty() {
            return;
        }
        let note = Some(self.note_input.read(cx).content.to_string());
        let status = self.new_status;
        let result = self.state.update(cx, |state, cx| {
            let r = state.link_commits(task, &commits, status, note);
            cx.notify();
            r
        });
        self.notice = Some(match result {
            Ok(0) => (format!("Already mapped to “{task_title}”"), false),
            Ok(1) => (format!("Mapped 1 commit to “{task_title}”"), false),
            Ok(n) => (format!("Mapped {n} commits to “{task_title}”"), false),
            Err(message) => (message, true),
        });
        self.note_input
            .update(cx, |input, cx| input.reset(window, cx));
        cx.notify();
    }

    fn chip(
        &self,
        label: impl Into<SharedString>,
        active: bool,
        id: impl Into<gpui::ElementId>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex_none()
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
            .child(label.into())
    }

    fn render_row(
        &self,
        visible_ix: usize,
        links: &HashMap<CommitKey, Vec<&CommitLink>>,
        task_titles: &HashMap<TaskId, String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(entry) = self
            .visible
            .get(visible_ix)
            .and_then(|&i| self.commits.get(i))
        else {
            return div().into_any_element();
        };
        let commit = &entry.commit;
        let key = (entry.repo_id, commit.hash.clone());
        let selected = self.selected.contains(&key);
        let badges = links.get(&key).cloned().unwrap_or_default();
        let extra = badges.len().saturating_sub(2);

        let short_hash = commit.hash[..7.min(commit.hash.len())].to_string();

        // Two lines like GitHub Desktop: the title on top (ellipsis when it
        // is long), the details underneath, task badges pinned right. The row
        // fills the list width so dividers and badges line up.
        div()
            .id(("commit-row", visible_ix as u64))
            .flex()
            .items_center()
            .gap(theme.space_3)
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(theme.space_2)
            .pr(theme.space_4)
            .border_b_1()
            .border_color(theme.border_subtle)
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.accent.opacity(0.14)))
            .when(!selected, |d| d.hover(move |s| s.bg(theme.surface_raised)))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, _window, cx| {
                    this.click_row(visible_ix, event.modifiers(), cx);
                }),
            )
            .child(
                // Selection bar: the accent shows which rows are selected
                // even when the tint is hard to see.
                div()
                    .flex_none()
                    .w(px(3.0))
                    .h(px(ROW_HEIGHT - 20.0))
                    .rounded_full()
                    .bg(if selected {
                        theme.accent
                    } else {
                        gpui::transparent_black()
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(theme.space_1)
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(commit.summary.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(
                                // Every row names its repo: a hash is only
                                // unique within one repo.
                                div()
                                    .flex_none()
                                    .px(theme.space_1)
                                    .rounded(theme.radius_sm)
                                    .border_1()
                                    .border_color(theme.border_light)
                                    .child(entry.repo_name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .max_w(px(180.0))
                                    .child(commit.author.clone()),
                            )
                            .child("·")
                            .child(div().flex_none().child(format_date(commit.time)))
                            .child("·")
                            .child(div().flex_none().font_family("Consolas").child(short_hash)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space_1)
                    .children(badges.iter().take(2).map(|link| {
                        let title = task_titles
                            .get(&link.task_id)
                            .cloned()
                            .unwrap_or_else(|| "task".to_string());
                        let color = status_color(&theme, link.status);
                        div()
                            .max_w(px(160.0))
                            .truncate()
                            .px(theme.space_2)
                            .py(px(1.0))
                            .rounded(theme.radius_sm)
                            .border_1()
                            .border_color(color.opacity(0.5))
                            .bg(color.opacity(0.10))
                            .text_xs()
                            .text_color(color)
                            .child(title)
                    }))
                    .when(extra > 0, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child(format!("+{extra}")),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_side_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let _ = window;
        let selected = self.selected_in_order();
        let (tasks, links): (Vec<(devspace_core::Task, usize)>, Vec<CommitLink>) = {
            let state = self.state.read(cx);
            (
                crate::flatten_tasks(&state.tasks),
                state.commit_links.clone(),
            )
        };
        let task_titles: HashMap<TaskId, String> =
            tasks.iter().map(|(t, _)| (t.id, t.title.clone())).collect();

        let mut panel = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDE_PANEL_WIDTH))
            .h_full()
            .min_h(px(0.0))
            .gap(theme.space_3)
            .p(theme.space_3)
            .bg(theme.surface)
            .border_l_1()
            .border_color(theme.border_subtle);

        if selected.is_empty() {
            return panel
                .child(
                    div()
                        .text_color(theme.text_secondary)
                        .text_sm()
                        .child("Select one or more commits to map them to a task. Ctrl-click adds, Shift-click selects a range."),
                )
                .into_any_element();
        }

        panel = panel.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_color(theme.text_primary).child(format!(
                    "{} commit{} selected",
                    selected.len(),
                    if selected.len() == 1 { "" } else { "s" }
                )))
                .child(
                    div()
                        .id("clear-selection")
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(theme.text_primary))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected.clear();
                            cx.notify();
                        }))
                        .child("Clear"),
                ),
        );

        // Detail + current mappings when exactly one commit is selected.
        if selected.len() == 1 {
            let key = &selected[0];
            if let Some(entry) = self
                .commits
                .iter()
                .find(|c| c.repo_id == key.0 && c.commit.hash == key.1)
            {
                panel =
                    panel.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space_1)
                            .p(theme.space_3)
                            .rounded(theme.radius_md)
                            .bg(theme.surface_raised)
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .child(entry.commit.summary.clone()),
                            )
                            .child(div().text_xs().text_color(theme.text_secondary).child(
                                format!(
                                    "{} · {} · {} · {}",
                                    entry.repo_name,
                                    entry.commit.author,
                                    format_date(entry.commit.time),
                                    &entry.commit.hash[..10.min(entry.commit.hash.len())]
                                ),
                            )),
                    );
            }
            let existing: Vec<&CommitLink> = links
                .iter()
                .filter(|l| l.repo_id == key.0 && l.hash == key.1)
                .collect();
            if !existing.is_empty() {
                panel = panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child("MAPPED TO"),
                    )
                    .children(existing.into_iter().map(|link| {
                        let id = link.id;
                        let color = status_color(&theme, link.status);
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(theme.space_2)
                            .px(theme.space_3)
                            .py(theme.space_2)
                            .rounded(theme.radius_sm)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w(px(0.0))
                                    .child(div().text_sm().text_color(theme.text_primary).child(
                                        task_titles.get(&link.task_id).cloned().unwrap_or_default(),
                                    ))
                                    .when_some(link.note.clone(), |d, note| {
                                        d.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_secondary)
                                                .child(note),
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
                                            .id(("link-status", id.0 as u64))
                                            .px(theme.space_2)
                                            .rounded(theme.radius_sm)
                                            .border_1()
                                            .border_color(color.opacity(0.5))
                                            .text_xs()
                                            .text_color(color)
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.state.update(cx, |state, cx| {
                                                    state.cycle_link_status(id);
                                                    cx.notify();
                                                });
                                            }))
                                            .child(status_label(link.status)),
                                    )
                                    .child(
                                        div()
                                            .id(("link-remove", id.0 as u64))
                                            .text_xs()
                                            .text_color(theme.text_secondary)
                                            .cursor_pointer()
                                            .hover(move |s| s.text_color(theme.danger))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.state.update(cx, |state, cx| {
                                                    state.unlink_commit(id);
                                                    cx.notify();
                                                });
                                            }))
                                            .child("✕"),
                                    ),
                            )
                    }));
            }
        }

        // Label + note for the commits about to be mapped.
        panel = panel
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child("LABEL"),
            )
            .child(div().flex().flex_wrap().gap(theme.space_1).children(
                SELECTABLE_STATUSES.iter().map(|&status| {
                    self.chip(
                        status_label(status),
                        self.new_status == status,
                        status_label(status),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_status = status;
                        cx.notify();
                    }))
                }),
            ))
            .child(self.note_input.clone());

        if let Some((message, is_error)) = &self.notice {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(if *is_error {
                        theme.danger
                    } else {
                        theme.success
                    })
                    .child(message.clone()),
            );
        }

        // Task picker: the project's whole task tree, indented by depth.
        panel = panel.child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child("MAP TO TASK"),
        );
        if tasks.is_empty() {
            panel = panel.child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child("This project has no tasks yet. Create one on the project page first."),
            );
        } else {
            panel = panel.child(
                div()
                    .id("task-picker")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_1)
                    .overflow_y_scroll()
                    .children(tasks.into_iter().map(|(task, depth)| {
                        let id = task.id;
                        let title = task.title.clone();
                        div()
                            .id(("map-task", id.0 as u64))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(theme.space_2)
                            .pl(theme.space_3 + px(16.0) * depth as f32)
                            .pr(theme.space_3)
                            .py(theme.space_2)
                            .rounded(theme.radius_sm)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .cursor_pointer()
                            .hover(move |s| s.bg(theme.surface_raised).border_color(theme.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.map_selection_to(id, title.clone(), window, cx);
                            }))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .child(task.title.clone()),
                            )
                            .child(div().text_xs().text_color(theme.accent).child("Map here"))
                    })),
            );
        }
        panel.into_any_element()
    }
}

impl Render for CommitBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let (repos, project_name) = {
            let state = self.state.read(cx);
            let name = state
                .selected_project
                .and_then(|id| state.projects.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_default();
            (state.repos.clone(), name)
        };

        let status_line = if self.commits.is_empty() && self.loading {
            "Loading history…".to_string()
        } else if self.visible.is_empty() {
            if self.applied_query.trim().is_empty() && !self.unassigned_only {
                "No commits. Link a repository on the project page to see its history.".to_string()
            } else {
                "No commits match.".to_string()
            }
        } else {
            format!(
                "{} commit{}{}{}",
                self.visible.len(),
                if self.visible.len() == 1 { "" } else { "s" },
                if self.has_more { " loaded" } else { "" },
                if self.loading {
                    " · loading more…"
                } else {
                    ""
                },
            )
        };

        let side_panel = self.render_side_panel(window, cx);
        let count = self.visible.len();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .min_h(px(0.0))
            .bg(theme.surface_solid)
            .child(
                // Header: back + title.
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .px(theme.space_4)
                    .pt(theme.space_3)
                    .child(
                        div()
                            .id("history-back")
                            .px(theme.space_2)
                            .rounded(theme.radius_sm)
                            .text_color(theme.text_secondary)
                            .cursor_pointer()
                            .hover(move |s| {
                                s.bg(theme.surface_raised).text_color(theme.text_primary)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(close) = &this.on_close {
                                    close(window, cx);
                                }
                            }))
                            .child("←"),
                    )
                    .child(
                        div()
                            .text_xl()
                            .text_color(theme.text_primary)
                            .child("Commit history"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_secondary)
                            .child(project_name),
                    ),
            )
            .child(
                // Toolbar: search + filters.
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space_2)
                    .px(theme.space_4)
                    .py(theme.space_3)
                    .child(self.search_input.clone())
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(theme.space_1)
                            .child(
                                self.chip("All repos", self.repo_filter.is_none(), "repo-all")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.repo_filter.take().is_some() {
                                            this.reload(cx);
                                        }
                                    })),
                            )
                            .children(repos.iter().map(|repo| {
                                let id = repo.id;
                                self.chip(
                                    repo.name.clone(),
                                    self.repo_filter == Some(id),
                                    ("repo-filter", id.0 as u64),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if this.repo_filter != Some(id) {
                                            this.repo_filter = Some(id);
                                            this.reload(cx);
                                        }
                                    },
                                ))
                            }))
                            .child(
                                self.chip(
                                    "Unassigned only",
                                    self.unassigned_only,
                                    "filter-unassigned",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.unassigned_only = !this.unassigned_only;
                                        this.refilter(cx);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(self.chip("Select all", false, "select-all").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.select_all_visible(cx);
                                }),
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(status_line),
                    )
                    .when(!self.unreadable.is_empty(), |d| {
                        d.child(div().text_xs().text_color(theme.warning).child(format!(
                            "Couldn't read history for: {}",
                            self.unreadable.join(", ")
                        )))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .border_t_1()
                    .border_color(theme.border_subtle)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(
                                uniform_list(
                                    "commit-list",
                                    count,
                                    cx.processor(|this, range: Range<usize>, _window, cx| {
                                        if this.has_more
                                            && range.end + PREFETCH_ROWS >= this.visible.len()
                                        {
                                            this.load_more(cx);
                                        }
                                        let (links, titles) = {
                                            let state = this.state.read(cx);
                                            let titles: HashMap<TaskId, String> = state
                                                .tasks
                                                .iter()
                                                .map(|t| (t.id, t.title.clone()))
                                                .collect();
                                            (state.commit_links.clone(), titles)
                                        };
                                        let by_commit = Self::links_by_commit(&links);
                                        range
                                            .map(|ix| this.render_row(ix, &by_commit, &titles, cx))
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .track_scroll(&self.scroll)
                                .w_full()
                                .flex_1()
                                .min_h(px(0.0)),
                            ),
                    )
                    .child(side_panel),
            )
    }
}
