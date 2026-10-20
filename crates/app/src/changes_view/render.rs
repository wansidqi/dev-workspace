//! Drawing the Changes view: file list, repo headers, diff viewer and the
//! commit box. Popovers are in `popovers.rs`.

use std::ops::Range;

use devspace_git::{ChangeKind, ChangedFile, DiffSide, LineKind};
use gpui::{div, px, uniform_list, AnyElement, Hsla, SharedString};

use super::*;
use crate::changes_model::{DiffRow, RepoRows, Row};
use crate::theme::{label_color, TypeScale, CODE_FONT};
use crate::ui::{
    back_button, button, checkbox, chip, empty_state, filter_chip, icon, icon_button, notice,
    page_header, segment, segmented, ButtonKind, CheckState, Icon,
};

const LIST_ROW_HEIGHT: f32 = 32.0;
const DIFF_ROW_HEIGHT: f32 = 22.0;
const LEFT_WIDTH: f32 = 400.0;
/// Width of one line-number column in the diff.
const GUTTER_WIDTH: f32 = 44.0;

fn kind_color(theme: &GlassTheme, kind: ChangeKind) -> Hsla {
    match kind {
        ChangeKind::Added | ChangeKind::Untracked => theme.success,
        ChangeKind::Modified | ChangeKind::Typechange => theme.warning,
        ChangeKind::Deleted | ChangeKind::Conflicted => theme.danger,
        ChangeKind::Renamed => theme.accent,
    }
}

fn tone_color(theme: &GlassTheme, tone: Tone) -> Hsla {
    match tone {
        Tone::Ok => theme.success,
        Tone::Warn => theme.warning,
        Tone::Err => theme.danger,
    }
}

/// Splits `src/app/main.rs` into (`src/app/`, `main.rs`).
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    }
}

impl Render for ChangesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let project_name = {
            let state = self.state.read(cx);
            state
                .selected_project
                .and_then(|id| state.projects.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_default()
        };

        let layout: Vec<RepoRows<'_>> = self
            .repos
            .iter()
            .map(|p| RepoRows {
                id: p.repo.id,
                status: p.status.as_ref(),
                collapsed: p.collapsed,
                failed: p.error.is_some(),
            })
            .collect();
        self.rows = changes_model::rows(&layout, self.repo_filter);
        drop(layout);

        let popover = self.render_popover(cx);
        let loading = self.repos.iter().any(|p| p.loading);

        let header = page_header(
            &theme,
            "Changes",
            Some(project_name.into()),
            Some(
                back_button(&theme, "changes-back")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(close) = &this.on_close {
                            close(window, cx);
                        }
                    }))
                    .into_any_element(),
            ),
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        button(
                            &theme,
                            "changes-all-branches",
                            ButtonKind::Secondary,
                            Some(Icon::GitBranch),
                            "Branches",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_branches(Scope::All, cx);
                        })),
                    )
                    .child(
                        button(
                            &theme,
                            "changes-refresh",
                            ButtonKind::Ghost,
                            Some(Icon::Refresh),
                            if loading { "Refreshing…" } else { "Refresh" },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_all(cx))),
                    )
                    .into_any_element(),
            ),
        );

        let notices = (!self.notice.is_empty()).then(|| {
            div()
                .flex()
                .items_start()
                .gap(theme.space_3)
                .px(theme.space_6)
                .py(theme.space_2)
                .border_b_1()
                .border_color(theme.border_subtle)
                .bg(theme.surface_hover)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .children(self.notice.iter().map(|line| {
                            notice(&theme, line.text.clone(), line.tone == Tone::Err)
                                .text_color(tone_color(&theme, line.tone))
                        })),
                )
                .child(
                    icon_button(
                        &theme,
                        Icon::X,
                        "changes-notice-dismiss",
                        theme.text_primary,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice.clear();
                        cx.notify();
                    })),
                )
        });

        let filter_chips = (self.repos.len() > 1).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.space_1)
                .px(theme.space_6)
                .py(theme.space_2)
                .child(
                    filter_chip(
                        &theme,
                        "changes-repo-all",
                        "All repos",
                        self.repo_filter.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.repo_filter = None;
                        cx.notify();
                    })),
                )
                .children(self.repos.iter().map(|pane| {
                    let id = pane.repo.id;
                    filter_chip(
                        &theme,
                        ("changes-repo-filter", id.0 as u64),
                        pane.repo.name.clone(),
                        self.repo_filter == Some(id),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.repo_filter = Some(id);
                        if this.amend && this.amend_target() != Some(id) {
                            this.amend = false;
                        }
                        cx.notify();
                    }))
                }))
        });

        let list = if self.repos.is_empty() {
            empty_state(
                &theme,
                Icon::GitBranch,
                "No repositories linked",
                "Link a git repository on the project page to see its changes here.",
                None,
            )
            .into_any_element()
        } else {
            uniform_list(
                "changes-files",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    range
                        .map(|ix| this.render_list_row(ix, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.list_scroll)
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .into_any_element()
        };

        let left = div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LEFT_WIDTH))
            .h_full()
            .min_h(px(0.0))
            .border_r_1()
            .border_color(theme.border_subtle)
            .child(list)
            .child(self.render_commit_box(cx))
            .children(popover);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .min_h(px(0.0))
            .child(header)
            .children(notices)
            .children(filter_chips)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(left)
                    .child(self.render_diff_panel(cx)),
            )
    }
}

impl ChangesView {
    fn render_list_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        match self.rows.get(ix).copied() {
            Some(Row::Repo(ri)) => self.render_repo_header(ri, cx),
            Some(Row::File { repo, file }) => self.render_file_row(repo, file, cx),
            Some(Row::Note(ri)) => self.render_note_row(ri),
            None => div().into_any_element(),
        }
    }

    fn render_note_row(&self, ri: usize) -> AnyElement {
        let theme = self.theme;
        let Some(pane) = self.repos.get(ri) else {
            return div().into_any_element();
        };
        let (text, color) = match (&pane.error, &pane.status) {
            (Some(error), _) => (error.clone(), theme.danger),
            (None, Some(_)) => ("No changes".to_string(), theme.text_muted),
            (None, None) => ("Reading…".to_string(), theme.text_muted),
        };
        div()
            .flex()
            .items_center()
            .h(px(LIST_ROW_HEIGHT))
            .pl(px(44.0))
            .pr(theme.space_3)
            .t_small()
            .text_color(color)
            .truncate()
            .child(text)
            .into_any_element()
    }

    fn render_repo_header(&self, ri: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(pane) = self.repos.get(ri) else {
            return div().into_any_element();
        };
        let id = pane.repo.id;
        let tint = label_color(&theme, &pane.repo.name);
        let files = pane.status.as_ref().map(|s| s.files.len()).unwrap_or(0);
        let check = match &pane.status {
            Some(s) if !s.files.is_empty() => {
                if s.files.iter().all(|f| f.is_fully_staged()) {
                    CheckState::On
                } else if s.files.iter().any(|f| f.staged.is_some()) {
                    CheckState::Mixed
                } else {
                    CheckState::Off
                }
            }
            _ => CheckState::Off,
        };

        let (branch_label, ahead, behind, in_progress) = match &pane.status {
            Some(s) => (
                match (&s.branch, &s.detached_at) {
                    (Some(b), _) => b.clone(),
                    (None, Some(h)) => format!("detached @ {h}"),
                    _ => "(unknown)".to_string(),
                },
                s.ahead,
                s.behind,
                s.in_progress,
            ),
            None => ("…".to_string(), 0, 0, None),
        };

        let branch_button = div()
            .id(("repo-branch", id.0 as u64))
            .flex()
            .flex_initial()
            .items_center()
            .gap(theme.space_1)
            .h(px(24.0))
            .max_w(px(150.0))
            .px(theme.space_2)
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(theme.border_subtle)
            .t_small()
            .t_strong()
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(move |s| s.bg(theme.surface_hover).text_color(theme.text_primary))
            .child(icon(Icon::GitBranch, 12.0, theme.text_muted))
            .child(div().truncate().child(branch_label))
            .when(ahead > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .t_code()
                        .t_micro()
                        .text_color(theme.accent)
                        .child(format!("↑{ahead}")),
                )
            })
            .when(behind > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .t_code()
                        .t_micro()
                        .text_color(theme.warning)
                        .child(format!("↓{behind}")),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_branches(Scope::Repo(id), cx);
            }));

        div()
            .id(("repo-head", id.0 as u64))
            .flex()
            .items_center()
            .gap(theme.space_2)
            .w_full()
            .h(px(LIST_ROW_HEIGHT + 4.0))
            .px(theme.space_3)
            .bg(theme.surface_hover)
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(
                icon_button(
                    &theme,
                    if pane.collapsed {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    ("repo-collapse", id.0 as u64),
                    theme.text_primary,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = this.pane_mut(id) {
                        p.collapsed = !p.collapsed;
                    }
                    cx.notify();
                })),
            )
            .child(
                checkbox(&theme, ("repo-check", id.0 as u64), check).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.toggle_repo(id, cx);
                        cx.stop_propagation();
                    },
                )),
            )
            .child(div().flex_none().size(px(8.0)).rounded_full().bg(tint))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .t_body()
                    .t_strong()
                    .text_color(theme.text_primary)
                    .child(pane.repo.name.clone()),
            )
            .when(files > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .t_small()
                        .t_code()
                        .text_color(theme.text_muted)
                        .child(files.to_string()),
                )
            })
            .when_some(in_progress, |d, op| {
                d.child(chip(&theme, op, theme.warning))
            })
            .child(branch_button)
            .child(
                icon_button(
                    &theme,
                    Icon::Archive,
                    ("repo-stash", id.0 as u64),
                    theme.text_primary,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_stashes(id, cx)))
                .when(!pane.stashes.is_empty(), |d| {
                    d.child(
                        div()
                            .t_micro()
                            .t_code()
                            .text_color(theme.accent)
                            .child(pane.stashes.len().to_string()),
                    )
                }),
            )
            .when(files > 0, |d| {
                d.child(
                    icon_button(
                        &theme,
                        Icon::Trash,
                        ("repo-discard", id.0 as u64),
                        theme.danger,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let files: Vec<String> = this
                            .pane(id)
                            .and_then(|p| p.status.as_ref())
                            .map(|s| s.files.iter().map(|f| f.path.clone()).collect())
                            .unwrap_or_default();
                        this.ask_discard_files(id, files, cx);
                    })),
                )
            })
            .into_any_element()
    }

    fn render_file_row(&self, ri: usize, fi: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(pane) = self.repos.get(ri) else {
            return div().into_any_element();
        };
        let Some(file) = pane.status.as_ref().and_then(|s| s.files.get(fi)) else {
            return div().into_any_element();
        };
        let repo = pane.repo.id;
        let path = file.path.clone();
        let is_selected = self.selected.as_ref() == Some(&(repo, path.clone()));
        let kind = file.display_kind();
        let color = kind_color(&theme, kind);
        let check = if file.is_fully_staged() {
            CheckState::On
        } else if file.is_partially_staged() {
            CheckState::Mixed
        } else {
            CheckState::Off
        };
        let (dir, name) = split_path(&file.path);
        let key = ((ri as u64) << 32) | fi as u64;

        div()
            .id(("file-row", key))
            .flex()
            .items_center()
            .gap(theme.space_2)
            .w_full()
            .h(px(LIST_ROW_HEIGHT))
            .pl(theme.space_6)
            .pr(theme.space_3)
            .cursor_pointer()
            .when(is_selected, |d| d.bg(theme.accent.opacity(0.10)))
            .when(!is_selected, |d| {
                d.hover(move |s| s.bg(theme.surface_hover))
            })
            .on_click({
                let path = path.clone();
                cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    // A double-click opens the file in the IDE; the first click of it
                    // has already selected the file.
                    if event.click_count() >= 2 {
                        this.open_file(repo, path.clone(), cx);
                    } else {
                        this.select_file(repo, path.clone(), cx);
                    }
                })
            })
            .child(checkbox(&theme, ("file-check", key), check).on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.toggle_file(repo, path.clone(), cx);
                    cx.stop_propagation();
                })
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .t_small()
                    .t_code()
                    .t_strong()
                    .text_color(color)
                    .child(kind.letter()),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .t_body()
                    .whitespace_nowrap()
                    .when(!dir.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(theme.text_muted)
                                .child(dir.to_string()),
                        )
                    })
                    .child(div().text_color(theme.text_primary).child(name.to_string()))
                    .when_some(file.old_path.clone(), |d, old| {
                        d.child(
                            div()
                                .ml(theme.space_2)
                                .t_small()
                                .text_color(theme.text_muted)
                                .child(format!("← {old}")),
                        )
                    }),
            )
            .child(
                icon_button(&theme, Icon::Code, ("file-open", key), theme.text_primary).on_click({
                    let path = path.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.open_file(repo, path.clone(), cx);
                        cx.stop_propagation();
                    })
                }),
            )
            .child(
                icon_button(&theme, Icon::Trash, ("file-discard", key), theme.danger).on_click({
                    let path = path.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.ask_discard_files(repo, vec![path.clone()], cx);
                        cx.stop_propagation();
                    })
                }),
            )
            .into_any_element()
    }

    // ---- commit box ----------------------------------------------------

    fn render_commit_box(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let targets = self.commit_targets();
        let names: Vec<String> = targets.iter().map(|id| self.repo_name(*id)).collect();
        let can_commit = self.can_commit(cx);
        let label = if self.busy {
            "Committing…".to_string()
        } else if self.amend {
            format!("Amend {}", names.first().cloned().unwrap_or_default())
        } else {
            match names.len() {
                0 => "Commit".to_string(),
                1 | 2 => format!("Commit to {}", crate::changes_model::join_names(&names)),
                n => format!("Commit to {n} repos"),
            }
        };
        let hint = if targets.is_empty() && !self.amend {
            Some("Tick files on the left to stage them.".to_string())
        } else if names.len() > 1 {
            Some(format!(
                "One commit in each of {}, same message.",
                crate::changes_model::join_names(&names)
            ))
        } else {
            None
        };

        let task_title = self.task_link.and_then(|id| {
            self.state
                .read(cx)
                .tasks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.title.clone())
        });
        let amend_available = self.amend_target().is_some();

        let confirm = self.render_confirm(cx);

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(theme.space_2)
            .p(theme.space_3)
            .border_t_1()
            .border_color(theme.border_subtle)
            .children(confirm)
            .child(self.summary.clone())
            .child(self.description.clone())
            .child(self.co_authors.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        div()
                            .id("changes-task-link")
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .items_center()
                            .gap(theme.space_2)
                            .h(px(24.0))
                            .px(theme.space_2)
                            .rounded(theme.radius_sm)
                            .border_1()
                            .border_color(if task_title.is_some() {
                                theme.accent.opacity(0.4)
                            } else {
                                theme.border_subtle
                            })
                            .t_small()
                            .text_color(if task_title.is_some() {
                                theme.accent
                            } else {
                                theme.text_secondary
                            })
                            .cursor_pointer()
                            .hover(move |s| s.bg(theme.surface_hover))
                            .child(icon(Icon::Link, 12.0, theme.text_muted))
                            .child(div().truncate().child(match &task_title {
                                Some(t) => format!("Linked to: {t}"),
                                None => "Link commit to a task".to_string(),
                            }))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.popover = Some(Popover::Tasks);
                                let focus = this.task_filter.read(cx).focus_handle.clone();
                                window.focus(&focus, cx);
                                cx.notify();
                            })),
                    )
                    .when(amend_available, |d| {
                        let on = self.amend;
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(theme.space_2)
                                .t_small()
                                .text_color(theme.text_secondary)
                                .child(
                                    checkbox(
                                        &theme,
                                        "changes-amend",
                                        if on { CheckState::On } else { CheckState::Off },
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.set_amend(!on, cx);
                                        },
                                    )),
                                )
                                .child("Amend last commit"),
                        )
                    }),
            )
            .when_some(hint, |d, hint| {
                d.child(div().t_small().text_color(theme.text_muted).child(hint))
            })
            .child({
                let b = button(
                    &theme,
                    "changes-commit",
                    ButtonKind::Primary,
                    Some(Icon::GitCommit),
                    label,
                )
                .w_full();
                if can_commit {
                    b.on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
                        .into_any_element()
                } else {
                    b.opacity(0.45).cursor_default().into_any_element()
                }
            })
            .into_any_element()
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let confirm = self.confirm.as_ref()?;
        let (text, action) = match confirm {
            Confirm::DiscardFiles { repo, files } => (
                format!(
                    "Discard all changes to {} file{} in {}? A backup copy is kept.",
                    files.len(),
                    if files.len() == 1 { "" } else { "s" },
                    self.repo_name(*repo)
                ),
                "Discard",
            ),
            Confirm::DiscardLines { count } => (
                format!(
                    "Discard {count} selected line{}? A copy of the file is saved first.",
                    if *count == 1 { "" } else { "s" }
                ),
                "Discard",
            ),
            Confirm::DeleteBranch { repo, name } => (
                format!(
                    "`{name}` has commits that aren't in {}'s current branch. Delete it anyway?",
                    self.repo_name(*repo)
                ),
                "Delete branch",
            ),
            Confirm::DropStash { repo, .. } => (
                format!(
                    "Drop this stash in {}? It can't be recovered.",
                    self.repo_name(*repo)
                ),
                "Drop",
            ),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .p(theme.space_3)
                .rounded(theme.radius_md)
                .border_1()
                .border_color(theme.danger.opacity(0.4))
                .bg(theme.danger.opacity(0.08))
                .child(div().t_small().text_color(theme.text_primary).child(text))
                .child(
                    div()
                        .flex()
                        .gap(theme.space_2)
                        .child(
                            button(
                                &theme,
                                "confirm-yes",
                                ButtonKind::Danger,
                                Some(Icon::Trash),
                                action,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_action(cx))),
                        )
                        .child(
                            button(&theme, "confirm-no", ButtonKind::Ghost, None, "Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if matches!(this.confirm, Some(Confirm::DiscardLines { .. })) {
                                        this.line_sel.clear();
                                    }
                                    this.confirm = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    // ---- diff ----------------------------------------------------------

    fn selected_file(&self) -> Option<(&RepoPane, &ChangedFile)> {
        let (repo, path) = self.selected.as_ref()?;
        let pane = self.pane(*repo)?;
        let file = pane
            .status
            .as_ref()?
            .files
            .iter()
            .find(|f| &f.path == path)?;
        Some((pane, file))
    }

    fn render_diff_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let shell = |content: AnyElement| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(content)
                .into_any_element()
        };
        let Some((pane, file)) = self.selected_file() else {
            let total: usize = self
                .repos
                .iter()
                .filter(|p| self.repo_filter.is_none_or(|f| f == p.repo.id))
                .map(|p| p.status.as_ref().map(|s| s.files.len()).unwrap_or(0))
                .sum();
            let (title, hint) = if total == 0 && self.repos.iter().all(|p| p.status.is_some()) {
                (
                    "No local changes",
                    "Nothing to commit in the linked repositories.",
                )
            } else {
                (
                    "Select a file",
                    "Pick a changed file to see its diff and stage parts of it.",
                )
            };
            return shell(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(empty_state(&theme, Icon::GitCommit, title, hint, None))
                    .into_any_element(),
            );
        };
        let repo_name = pane.repo.name.clone();
        let (has_unstaged, has_staged) = (file.unstaged.is_some(), file.staged.is_some());
        let path = file.path.clone();
        let (dir, name) = split_path(&path);

        let (added, removed) = self.diff.as_ref().map_or((0, 0), |d| {
            d.diff
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .fold((0, 0), |(a, r), l| match l.kind {
                    LineKind::Added => (a + 1, r),
                    LineKind::Removed => (a, r + 1),
                    _ => (a, r),
                })
        });

        let side_switch = (has_unstaged && has_staged).then(|| {
            segmented(&theme)
                .child(
                    segment(
                        &theme,
                        "side-unstaged",
                        "Unstaged",
                        self.side == DiffSide::Unstaged,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_side(DiffSide::Unstaged, cx))),
                )
                .child(
                    segment(
                        &theme,
                        "side-staged",
                        "Staged",
                        self.side == DiffSide::Staged,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_side(DiffSide::Staged, cx))),
                )
        });

        let header = div()
            .flex()
            .items_center()
            .gap(theme.space_3)
            .flex_none()
            .h(px(44.0))
            .px(theme.space_4)
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(chip(
                &theme,
                repo_name,
                label_color(&theme, &pane.repo.name),
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .t_body()
                    .t_code()
                    .whitespace_nowrap()
                    .child(div().text_color(theme.text_muted).child(dir.to_string()))
                    .child(
                        div()
                            .text_color(theme.text_primary)
                            .t_strong()
                            .child(name.to_string()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .t_small()
                    .t_code()
                    .flex()
                    .gap(theme.space_2)
                    .child(div().text_color(theme.success).child(format!("+{added}")))
                    .child(div().text_color(theme.danger).child(format!("−{removed}"))),
            )
            .children(side_switch);

        let selection_bar = (!self.line_sel.is_empty() && self.diff.is_some()).then(|| {
            let n = self.line_sel.len();
            let staged_side = self.side == DiffSide::Staged;
            div()
                .flex()
                .items_center()
                .gap(theme.space_2)
                .flex_none()
                .h(px(40.0))
                .px(theme.space_4)
                .border_b_1()
                .border_color(theme.border_subtle)
                .bg(theme.accent.opacity(0.08))
                .child(
                    div()
                        .flex_1()
                        .t_small()
                        .text_color(theme.text_primary)
                        .child(format!(
                            "{n} line{} selected",
                            if n == 1 { "" } else { "s" }
                        )),
                )
                .child(
                    button(
                        &theme,
                        "lines-stage",
                        ButtonKind::Secondary,
                        Some(Icon::Plus),
                        if staged_side {
                            "Unstage lines"
                        } else {
                            "Stage lines"
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let sel = this.line_sel.clone();
                        this.apply_lines(
                            if staged_side {
                                LineOp::Unstage
                            } else {
                                LineOp::Stage
                            },
                            sel,
                            cx,
                        );
                    })),
                )
                .when(!staged_side, |d| {
                    d.child(
                        button(
                            &theme,
                            "lines-discard",
                            ButtonKind::Danger,
                            Some(Icon::Trash),
                            "Discard lines",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm = Some(Confirm::DiscardLines {
                                count: this.line_sel.len(),
                            });
                            cx.notify();
                        })),
                    )
                })
                .child(
                    button(&theme, "lines-clear", ButtonKind::Ghost, None, "Clear").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.line_sel.clear();
                            cx.notify();
                        }),
                    ),
                )
        });

        let body: AnyElement = if let Some(error) = &self.diff_error {
            div()
                .p(theme.space_4)
                .child(notice(&theme, error.clone(), true))
                .into_any_element()
        } else if let Some(loaded) = &self.diff {
            if loaded.diff.binary {
                note_box(
                    &theme,
                    "Binary file",
                    "This file can only be staged as a whole.",
                )
            } else if loaded.rows.is_empty() {
                note_box(
                    &theme,
                    "No changes on this side",
                    "Pick the other side above, or another file.",
                )
            } else {
                let truncated = loaded.diff.truncated;
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(
                        uniform_list(
                            "diff-rows",
                            loaded.rows.len(),
                            cx.processor(|this, range: Range<usize>, _window, cx| {
                                range.map(|ix| this.render_diff_row(ix, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.diff_scroll)
                        .w_full()
                        .flex_1()
                        .min_h(px(0.0)),
                    )
                    .when(truncated, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .p(theme.space_3)
                                .t_small()
                                .text_color(theme.warning)
                                .child("This diff is long; only the first part is shown. Stage the whole file instead."),
                        )
                    })
                    .into_any_element()
            }
        } else {
            note_box(
                &theme,
                if self.diff_loading {
                    "Loading diff…"
                } else {
                    "No diff"
                },
                "",
            )
        };

        shell(
            div()
                .flex()
                .flex_col()
                .size_full()
                .min_h(px(0.0))
                .child(header)
                .children(selection_bar)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_diff_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(loaded) = &self.diff else {
            return div().into_any_element();
        };
        match loaded.rows.get(ix).copied() {
            Some(DiffRow::Hunk(h)) => {
                let hunk = &loaded.diff.hunks[h];
                let staged_side = loaded.side == DiffSide::Staged;
                div()
                    .id(("diff-hunk", h as u64))
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .w_full()
                    .h(px(DIFF_ROW_HEIGHT + 6.0))
                    .px(theme.space_3)
                    .bg(theme.surface_sunken)
                    .border_y_1()
                    .border_color(theme.border_subtle)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(hunk.header.clone()),
                    )
                    .child(
                        button(
                            &theme,
                            ("hunk-stage", h as u64),
                            ButtonKind::Ghost,
                            Some(if staged_side { Icon::Minus } else { Icon::Plus }),
                            if staged_side {
                                "Unstage hunk"
                            } else {
                                "Stage hunk"
                            },
                        )
                        .h(px(22.0))
                        .on_click(cx.listener(move |this, _, _, cx| this.stage_hunk(h, cx))),
                    )
                    .when(!staged_side, |d| {
                        d.child(
                            icon_button(
                                &theme,
                                Icon::Trash,
                                ("hunk-discard", h as u64),
                                theme.danger,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.discard_hunk(h, cx))),
                        )
                    })
                    .into_any_element()
            }
            Some(DiffRow::Line { hunk, line }) => {
                let l = &loaded.diff.hunks[hunk].lines[line];
                let changed = matches!(l.kind, LineKind::Added | LineKind::Removed);
                let picked = self.line_sel.contains(&(hunk, line));
                let (bg, marker, marker_color) = match l.kind {
                    LineKind::Added => (theme.success.opacity(0.12), "+", theme.success),
                    LineKind::Removed => (theme.danger.opacity(0.12), "−", theme.danger),
                    _ => (gpui::transparent_black(), " ", theme.text_muted),
                };
                let text: SharedString = l
                    .text
                    .replace('\t', "    ")
                    .trim_end_matches('\r')
                    .to_string()
                    .into();
                let key = ((hunk as u64) << 32) | line as u64;
                div()
                    .id(("diff-line", key))
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(DIFF_ROW_HEIGHT))
                    .bg(if picked {
                        theme.accent.opacity(0.22)
                    } else {
                        bg
                    })
                    .font_family(CODE_FONT)
                    .t_small()
                    .when(changed, |d| {
                        d.cursor_pointer().on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, _window, cx| {
                                this.click_line(hunk, line, event.modifiers().shift, cx);
                            },
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(px(28.0))
                            .when(changed, |d| {
                                d.child(checkbox(
                                    &theme,
                                    ("line-check", key),
                                    if picked {
                                        CheckState::On
                                    } else {
                                        CheckState::Off
                                    },
                                ))
                            }),
                    )
                    .child(gutter(&theme, l.old_lineno))
                    .child(gutter(&theme, l.new_lineno))
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.0))
                            .text_color(marker_color)
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme.text_primary)
                            .child(text),
                    )
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}

fn gutter(theme: &GlassTheme, n: Option<u32>) -> gpui::Div {
    div()
        .flex_none()
        .w(px(GUTTER_WIDTH))
        .pr(theme.space_2)
        .text_right()
        .text_color(theme.text_muted)
        .child(n.map(|n| n.to_string()).unwrap_or_default())
}

fn note_box(theme: &GlassTheme, title: &str, hint: &str) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .child(empty_state(
            theme,
            Icon::Info,
            title.to_string(),
            hint.to_string(),
            None,
        ))
        .into_any_element()
}
