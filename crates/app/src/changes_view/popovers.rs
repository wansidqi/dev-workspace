//! The three popovers of the Changes view: branches (for one repo or all of
//! them), stashes, and the task picker for linking a commit.

use std::collections::BTreeMap;

use gpui::{div, px, AnyElement};

use super::*;
use crate::commit_browser::PICKER_TREE;
use crate::motion;
use crate::task_tree::{filter_tree, tree_guide_lines, tree_guides};
use crate::theme::TypeScale;
/// Recently used branches listed above the rest.
const RECENT_SHOWN: usize = 5;

/// A small heading between groups of the branch list.
fn heading(theme: &GlassTheme, text: &'static str) -> AnyElement {
    div()
        .pt(theme.space_2)
        .pb(px(2.0))
        .px(theme.space_2)
        .child(section_label(theme, text))
        .into_any_element()
}

use crate::ui::{
    button, chip, icon, icon_button, section_label, segment, segmented, ButtonKind, Icon,
};

const POPOVER_WIDTH: f32 = 372.0;

impl ChangesView {
    pub(super) fn render_popover(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let content = match self.popover.as_ref()? {
            Popover::Branches(scope) => self.branches_popover(*scope, cx),
            Popover::Tasks => self.tasks_popover(cx),
        };
        Some(
            gpui::deferred(motion::enter_by(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left(px(8.0))
                    .w(px(POPOVER_WIDTH))
                    .flex()
                    .flex_col()
                    .gap(theme.space_2)
                    .p(theme.space_3)
                    .rounded(theme.radius_md)
                    .border_1()
                    .border_color(theme.border_light)
                    .bg(theme.surface_solid)
                    .shadow_lg()
                    .occlude()
                    // A click anywhere else closes it.
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popover(cx)))
                    .child(content),
                "changes-popover-enter",
                motion::FAST,
                0.0,
            ))
            .with_priority(10)
            .into_any_element(),
        )
    }

    fn popover_title(&self, title: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .items_center()
            .gap(theme.space_2)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .t_body()
                    .t_strong()
                    .text_color(theme.text_primary)
                    .child(title),
            )
            .child(
                icon_button(&theme, Icon::X, "popover-close", theme.text_primary)
                    .on_click(cx.listener(|this, _, _, cx| this.close_popover(cx))),
            )
            .into_any_element()
    }

    // ---- branches ------------------------------------------------------

    fn branches_popover(&mut self, scope: Scope, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let query = self.branch_filter.read(cx).content.to_lowercase();
        let panes: Vec<&RepoPane> = self
            .repos
            .iter()
            .filter(|p| match scope {
                Scope::All => true,
                Scope::Repo(id) => p.repo.id == id,
            })
            .collect();
        let total = panes.len();

        // Branch name -> repos that are on it now / that have it.
        let mut known: BTreeMap<String, usize> = BTreeMap::new();
        let mut current: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for pane in &panes {
            for branch in &pane.branches {
                *known.entry(branch.clone()).or_default() += 1;
            }
            if let Some(b) = pane.status.as_ref().and_then(|s| s.branch.clone()) {
                current.entry(b).or_default().push(pane.repo.name.clone());
            }
        }
        // The branches checked out now, then the recently used ones (newest
        // first, across the scope's repos), then everything else A-Z.
        let mut latest: BTreeMap<String, i64> = BTreeMap::new();
        for pane in &panes {
            for r in &pane.recent {
                let t = latest.entry(r.name.clone()).or_insert(r.time);
                *t = (*t).max(r.time);
            }
        }
        let matches = |n: &String| query.is_empty() || n.to_lowercase().contains(&query);
        let mut checked_out: Vec<String> = current
            .keys()
            .filter(|n| known.contains_key(*n) && matches(n))
            .cloned()
            .collect();
        checked_out.sort_by_key(|n| n.to_lowercase());
        let mut recent: Vec<String> = latest
            .iter()
            .filter(|(n, _)| known.contains_key(*n) && !current.contains_key(*n) && matches(n))
            .map(|(n, _)| n.clone())
            .collect();
        recent.sort_by_key(|n| std::cmp::Reverse(latest[n]));
        recent.truncate(RECENT_SHOWN);
        let mut others: Vec<String> = known
            .keys()
            .filter(|n| !current.contains_key(*n) && !recent.contains(n) && matches(n))
            .cloned()
            .collect();
        others.sort_by_key(|n| n.to_lowercase());
        // Headings only when the list isn't being searched.
        let sectioned = query.is_empty() && !recent.is_empty();
        let (n_current, n_recent) = (checked_out.len(), recent.len());
        let names: Vec<String> = checked_out
            .into_iter()
            .chain(recent)
            .chain(others)
            .collect();

        let title = match scope {
            Scope::All => format!("Branches · all {total} repos"),
            Scope::Repo(id) => format!("Branches · {}", self.repo_name(id)),
        };

        let rows = names
            .into_iter()
            .enumerate()
            .map(|(ix, name)| {
                let here = current.get(&name).cloned().unwrap_or_default();
                let have = known.get(&name).copied().unwrap_or(0);
                let is_current_single = matches!(scope, Scope::Repo(_)) && !here.is_empty();
                let switch_name = name.clone();
                div()
                    .id(("branch-row", ix as u64))
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .w_full()
                    .h(px(30.0))
                    .px(theme.space_2)
                    .rounded(theme.radius_sm)
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_hover))
                    .child(icon(
                        Icon::GitBranch,
                        13.0,
                        if here.is_empty() {
                            theme.text_muted
                        } else {
                            theme.accent
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .t_body()
                            .text_color(if here.is_empty() {
                                theme.text_primary
                            } else {
                                theme.accent
                            })
                            .child(name.clone()),
                    )
                    .when(!here.is_empty() && matches!(scope, Scope::All), |d| {
                        // Which repos are on it, kept short so the name keeps its room.
                        let label = if here.len() <= 2 {
                            here.join(", ")
                        } else {
                            format!("{} repos", here.len())
                        };
                        d.child(
                            div()
                                .flex_none()
                                .max_w(px(110.0))
                                .truncate()
                                .t_micro()
                                .text_color(theme.text_muted)
                                .child(label),
                        )
                    })
                    .when(is_current_single, |d| {
                        d.child(chip(&theme, "current", theme.accent))
                    })
                    .when(matches!(scope, Scope::All) && have < total, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .t_micro()
                                .t_code()
                                .text_color(theme.warning)
                                .child(format!("{have}/{total}")),
                        )
                    })
                    .when_some(
                        match scope {
                            Scope::Repo(id) if !is_current_single => Some(id),
                            _ => None,
                        },
                        |d, repo| {
                            let rename_from = name.clone();
                            let delete_name = name.clone();
                            d.child(
                                icon_button(
                                    &theme,
                                    Icon::Pencil,
                                    ("branch-rename", ix as u64),
                                    theme.text_primary,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.start_rename(repo, rename_from.clone(), cx);
                                        let focus = this.branch_input.read(cx).focus_handle.clone();
                                        window.focus(&focus, cx);
                                        cx.stop_propagation();
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    &theme,
                                    Icon::Trash,
                                    ("branch-delete", ix as u64),
                                    theme.danger,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.delete_branch(repo, delete_name.clone(), false, cx);
                                        cx.stop_propagation();
                                    },
                                )),
                            )
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switch_branch(scope, switch_name.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        // Put the headings between the groups.
        let mut children: Vec<AnyElement> = Vec::with_capacity(rows.len() + 2);
        for (i, row) in rows.into_iter().enumerate() {
            if sectioned && i == n_current {
                children.push(heading(&theme, "Recent"));
            }
            if sectioned && i == n_current + n_recent {
                children.push(heading(&theme, "Other branches"));
            }
            children.push(row);
        }

        let task_branch = self.task_link.and_then(|id| {
            self.state
                .read(cx)
                .tasks
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.branch.clone().map(|b| (t.title.clone(), b)))
        });

        let creating = match &self.branch_input_mode {
            BranchInput::Create => match scope {
                Scope::All => format!("Enter creates it in all {total} repos."),
                Scope::Repo(id) => format!("Enter creates it in {}.", self.repo_name(id)),
            },
            BranchInput::Rename { from, .. } => format!("Enter renames `{from}`."),
        };

        let mode = self.switch_mode;
        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(self.popover_title(title, cx))
            .child(self.branch_filter.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space_2)
                    .child(
                        div()
                            .t_small()
                            .text_color(theme.text_secondary)
                            .child("Uncommitted changes"),
                    )
                    .child(
                        segmented(&theme)
                            .child(
                                segment(
                                    &theme,
                                    "mode-bring",
                                    "Bring them",
                                    mode == SwitchMode::Bring,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.switch_mode = SwitchMode::Bring;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                segment(
                                    &theme,
                                    "mode-stash",
                                    "Stash them",
                                    mode == SwitchMode::Stash,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.switch_mode = SwitchMode::Stash;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .when_some(task_branch, |d, (title, branch)| {
                d.child(
                    button(
                        &theme,
                        "use-task-branch",
                        ButtonKind::Secondary,
                        Some(Icon::Link),
                        format!("Use task branch `{branch}` in all repos"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.use_task_branch(branch.clone(), cx);
                    })),
                )
                .child(
                    div()
                        .t_micro()
                        .text_color(theme.text_muted)
                        .child(format!("Branch of “{title}”. Created where it is missing.")),
                )
            })
            .child(
                div()
                    .id("branch-list")
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .max_h(px(260.0))
                    .overflow_y_scroll()
                    .children(children),
            )
            .child(section_label(&theme, "New branch"))
            .child(self.branch_input.clone())
            .child(div().t_micro().text_color(theme.text_muted).child(creating))
            .into_any_element()
    }

    // ---- task picker ---------------------------------------------------

    fn tasks_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let query = self.task_filter.read(cx).content.to_string();
        let tasks = crate::flatten_tasks(&self.state.read(cx).tasks);
        let current = self.task_link;

        // The same tree the commit browser's picker draws: connector lines
        // hang each subtask off its parent, and a search keeps the parents of
        // every match so the result is still a tree.
        let rows = filter_tree(tasks, &query);
        let depths: Vec<usize> = rows.iter().map(|(_, depth, _)| *depth).collect();
        let empty = rows.is_empty();
        let rows = rows
            .into_iter()
            .enumerate()
            .map(|(ix, (task, depth, matched))| {
                let id = task.id;
                let guides = tree_guides(&depths, ix);
                div()
                    .id(("task-pick", id.0 as u64))
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space_1)
                    .h(px(PICKER_TREE.row_height))
                    .pl(theme.space_2 + px(PICKER_TREE.indent * depth as f32))
                    .pr(theme.space_2)
                    .rounded(theme.radius_sm)
                    .cursor_pointer()
                    .when(current == Some(id), |d| d.bg(theme.accent.opacity(0.10)))
                    .hover(move |s| s.bg(theme.surface_hover))
                    .children(tree_guide_lines(
                        &theme,
                        &guides,
                        PICKER_TREE,
                        theme.space_2,
                    ))
                    // Empty slot the tree lines hang from, instead of a status icon.
                    .child(div().flex_none().w(px(PICKER_TREE.icon)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .t_body()
                            .when(depth == 0, |d| d.t_strong())
                            .text_color(if matched {
                                theme.text_primary
                            } else {
                                // Shown only as the parent of a match.
                                theme.text_muted
                            })
                            .child(task.title.clone()),
                    )
                    .when_some(task.branch.clone(), |d, b| {
                        d.child(
                            div()
                                .flex_none()
                                .max_w(px(110.0))
                                .truncate()
                                .t_micro()
                                .t_code()
                                .text_color(theme.text_muted)
                                .child(b),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_task_link(Some(id), cx);
                    }))
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(self.popover_title("Link new commits to a task".to_string(), cx))
            .child(self.task_filter.clone())
            .child(
                div()
                    .id("task-none")
                    .flex()
                    .items_center()
                    .h(px(28.0))
                    .px(theme.space_2)
                    .rounded(theme.radius_sm)
                    .t_body()
                    .text_color(theme.text_secondary)
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_hover))
                    .child("No task")
                    .on_click(cx.listener(|this, _, _, cx| this.set_task_link(None, cx))),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .px(theme.space_2)
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child("No tasks match."),
                )
            })
            .child(
                div()
                    .id("task-list")
                    .flex()
                    .flex_col()
                    .max_h(px(300.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}
