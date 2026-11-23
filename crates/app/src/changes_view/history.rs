//! The History and Merge popovers of the Changes view: rewriting the newest
//! commits of a branch (squash, fixup, reorder, reword, drop), reverting,
//! cherry-picking onto another branch, undoing, and merging a branch in.
//! (Phases 3 and 4 of CLAUDE.md "Git client".)

use std::collections::HashSet;

use gpui::{div, px, AnyElement, Hsla};

use super::*;
use crate::changes_model::{self, PlanAction, PlanItem};
use crate::theme::{TypeScale, CODE_FONT};
use crate::ui::{
    button, checkbox, chip, icon, icon_button, section_label, ButtonKind, CheckState, Icon,
};

fn action_color(theme: &GlassTheme, action: &PlanAction) -> Hsla {
    match action {
        PlanAction::Pick => theme.text_muted,
        PlanAction::Squash | PlanAction::Fixup => theme.accent,
        PlanAction::Drop => theme.danger,
        PlanAction::Reword(_) => theme.warning,
    }
}

/// What is being dragged from the commit list: the commits (oldest first,
/// ready for `git cherry-pick`) and a label for the drag preview.
pub(crate) struct DraggedCommits {
    pub hashes: Vec<String>,
    pub label: String,
}

/// The little pill that follows the pointer while commits are dragged.
struct DragPreview {
    label: String,
    theme: GlassTheme,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .px(theme.space_3)
            .py(theme.space_1)
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(theme.accent)
            .bg(theme.surface_solid)
            .t_small()
            .text_color(theme.text_primary)
            .child(self.label.clone())
    }
}

impl ChangesView {
    // ---- shared --------------------------------------------------------

    /// Loads a repo's branch names (for the cherry-pick and merge lists).
    fn load_branches(&mut self, id: RepoId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(id) else { return };
        let repo = pane.repo.clone();
        self.run(
            cx,
            move || git::list_branches(&repo).unwrap_or_default(),
            move |this, branches, cx| {
                if let Some(p) = this.pane_mut(id) {
                    p.branches = branches;
                }
                cx.notify();
            },
        );
    }

    fn current_branch_of(&self, id: RepoId) -> Option<String> {
        self.pane(id)
            .and_then(|p| p.status.as_ref())
            .and_then(|s| s.branch.clone())
    }

    // ---- history -------------------------------------------------------

    pub(crate) fn open_history(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        self.popover = Some(Popover::History);
        self.confirm = None;
        self.load_branches(repo, cx);
        self.history = Some(HistoryState {
            repo,
            loading: true,
            error: None,
            original: Vec::new(),
            plan: Vec::new(),
            picked: HashSet::new(),
            rewording: None,
            choosing_target: false,
        });
        self.run(
            cx,
            move || git::rebase_candidates(&path, HISTORY_LIMIT),
            move |this, result, cx| {
                if let Some(h) = this.history.as_mut().filter(|h| h.repo == repo) {
                    h.loading = false;
                    match result {
                        Ok(list) => {
                            h.original = list.iter().map(|c| c.commit.hash.clone()).collect();
                            h.plan = list
                                .into_iter()
                                .map(|c| PlanItem {
                                    hash: c.commit.hash,
                                    summary: c.commit.summary,
                                    unpushed: c.unpushed,
                                    action: PlanAction::Pick,
                                })
                                .collect();
                        }
                        Err(e) => h.error = Some(e.to_string()),
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn cycle_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(h) = self.history.as_mut() {
            changes_model::cycle_action(&mut h.plan, ix);
            h.rewording = None;
        }
        cx.notify();
    }

    fn move_row(&mut self, ix: usize, up: bool, cx: &mut Context<Self>) {
        if let Some(h) = self.history.as_mut() {
            changes_model::move_item(&mut h.plan, ix, up);
            h.rewording = None;
        }
        cx.notify();
    }

    fn start_reword(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_mut() else {
            return;
        };
        let Some(item) = h.plan.get(ix) else { return };
        let text = match &item.action {
            PlanAction::Reword(m) => m.clone(),
            _ => item.summary.clone(),
        };
        h.rewording = Some(ix);
        self.reword_input.update(cx, |i, cx| {
            i.set_text_selected(text);
            cx.notify();
        });
        cx.notify();
    }

    /// Enter in the reword box: the commit keeps its place with a new message.
    pub(super) fn apply_reword(&mut self, text: String, cx: &mut Context<Self>) {
        let message = text.trim().to_owned();
        if message.is_empty() {
            return;
        }
        if let Some(h) = self.history.as_mut() {
            if let Some(ix) = h.rewording.take() {
                if let Some(item) = h.plan.get_mut(ix) {
                    item.action = if message == item.summary {
                        PlanAction::Pick
                    } else {
                        PlanAction::Reword(message)
                    };
                }
            }
        }
        cx.notify();
    }

    fn toggle_pick(&mut self, hash: String, cx: &mut Context<Self>) {
        if let Some(h) = self.history.as_mut() {
            if !h.picked.remove(&hash) {
                h.picked.insert(hash);
            }
        }
        cx.notify();
    }

    /// "Rewrite history": runs the plan, asking first when it reaches commits
    /// that are already pushed.
    fn ask_rebase(&mut self, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_ref() else {
            return;
        };
        let repo = h.repo;
        if !changes_model::plan_changes_history(&h.plan, &h.original) {
            return;
        }
        let pushed = changes_model::plan_touches_pushed(&h.plan, &h.original);
        if pushed {
            self.confirm = Some(Confirm::RunRebase { repo });
            cx.notify();
        } else {
            self.run_rebase(repo, cx);
        }
    }

    pub(crate) fn run_rebase(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_ref().filter(|h| h.repo == repo) else {
            return;
        };
        let entries = changes_model::plan_entries(&h.plan);
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.close_popover(cx);
        if !self.begin_work(&[repo], "Rewriting history…", cx) {
            return;
        }
        self.run(
            cx,
            move || git::rebase_interactive(&path, &entries),
            move |this, result, cx| {
                this.end_work(&[repo]);
                match result {
                    Ok(git::RebaseOutcome::Done) => {
                        this.set_notice(
                            vec![NoticeLine::ok(format!(
                                "{name}: history rewritten. If it was already pushed, force push to update the remote"
                            ))],
                            cx,
                        );
                        this.relink_repo(repo, cx);
                    }
                    Ok(git::RebaseOutcome::Conflicts(n)) => this.set_notice(
                        vec![NoticeLine::warn(format!(
                            "{name}: stopped on {n} conflicting file{}. Resolve them and continue, or abort",
                            if n == 1 { "" } else { "s" }
                        ))],
                        cx,
                    ),
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    fn revert_row(&mut self, hash: String, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_ref() else {
            return;
        };
        let repo = h.repo;
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.close_popover(cx);
        if !self.begin_work(&[repo], "Reverting…", cx) {
            return;
        }
        self.run(
            cx,
            move || git::revert_commit(&path, &hash),
            move |this, result, cx| {
                this.end_work(&[repo]);
                match result {
                    Ok(false) => this.set_notice(
                        vec![NoticeLine::ok(format!(
                            "{name}: added a commit that undoes it"
                        ))],
                        cx,
                    ),
                    Ok(true) => this.set_notice(
                        vec![NoticeLine::warn(format!(
                            "{name}: the revert conflicts. Resolve the files and continue, or abort"
                        ))],
                        cx,
                    ),
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    fn choose_cherry_target(&mut self, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_mut() else {
            return;
        };
        h.choosing_target = true;
        let repo = h.repo;
        self.load_branches(repo, cx);
        cx.notify();
    }

    /// Switches to `target` (carrying or stashing changes per the branch
    /// popover's setting) and applies the ticked commits there, oldest first.
    fn cherry_pick_to(&mut self, target: String, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_ref() else {
            return;
        };
        // The plan is newest first; git wants oldest first.
        let hashes: Vec<String> = h
            .plan
            .iter()
            .rev()
            .filter(|p| h.picked.contains(&p.hash))
            .map(|p| p.hash.clone())
            .collect();
        self.cherry_pick_hashes(target, hashes, cx);
    }

    /// Applies `hashes` (oldest first) onto `target`: the ticked commits, or
    /// the ones dropped on a branch.
    fn cherry_pick_hashes(&mut self, target: String, hashes: Vec<String>, cx: &mut Context<Self>) {
        let Some(h) = self.history.as_ref() else {
            return;
        };
        if hashes.is_empty() {
            return;
        }
        let repo = h.repo;
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        let mode = self.switch_mode;
        self.close_popover(cx);
        if !self.begin_work(&[repo], &format!("Cherry-picking onto {target}…"), cx) {
            return;
        }
        self.run(
            cx,
            {
                let target = target.clone();
                move || {
                    git::switch_branch(&path, &target, mode)?;
                    git::cherry_pick(&path, &hashes).map(|conflicts| (conflicts, hashes.len()))
                }
            },
            move |this, result, cx| {
                this.end_work(&[repo]);
                match result {
                    Ok((false, n)) => this.set_notice(
                        vec![NoticeLine::ok(format!(
                            "{name}: applied {n} commit{} to {target}. You are on {target} now",
                            if n == 1 { "" } else { "s" }
                        ))],
                        cx,
                    ),
                    Ok((true, _)) => this.set_notice(
                        vec![NoticeLine::warn(format!(
                            "{name}: the cherry-pick conflicts on {target}. Resolve the files and continue, or abort"
                        ))],
                        cx,
                    ),
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    pub(super) fn history_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(h) = self.history.as_ref() else {
            return div().into_any_element();
        };
        let repo = h.repo;
        let title = format!("History · {}", self.repo_name(repo));
        let current = self.current_branch_of(repo);
        let in_operation = self
            .pane(repo)
            .and_then(|p| p.status.as_ref())
            .is_some_and(|s| s.in_progress.is_some());
        let dirty = self
            .pane(repo)
            .and_then(|p| p.status.as_ref())
            .is_some_and(|s| !s.files.is_empty());
        let h = self.history.as_ref().expect("checked above");
        let changed = changes_model::plan_changes_history(&h.plan, &h.original);
        let pushed = changes_model::plan_touches_pushed(&h.plan, &h.original);
        let picked = h.picked.len();

        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.space_2)
            .child(
                button(
                    &theme,
                    "history-merge",
                    ButtonKind::Secondary,
                    Some(Icon::GitMerge),
                    "Merge a branch…",
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_merge(repo, cx))),
            )
            .child(
                button(
                    &theme,
                    "history-undo",
                    ButtonKind::Secondary,
                    Some(Icon::ArrowLeft),
                    "Undo last commit",
                )
                .on_click(cx.listener(move |this, _, _, cx| this.ask_undo(repo, cx))),
            )
            .when(self.github_enabled(cx), |d| {
                d.child(
                    button(
                        &theme,
                        "history-prs",
                        ButtonKind::Secondary,
                        Some(Icon::Link),
                        "Pull requests…",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_pull_requests(repo, cx))),
                )
            })
            .child(
                button(
                    &theme,
                    "history-force-push",
                    ButtonKind::Ghost,
                    Some(Icon::Upload),
                    "Force push",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_popover(cx);
                    this.confirm = Some(Confirm::ForcePush { repo });
                    cx.notify();
                })),
            );

        let body: AnyElement = if h.choosing_target {
            let rows = self
                .pane(repo)
                .map(|p| p.branches.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|b| Some(b) != current.as_ref())
                .enumerate()
                .map(|(ix, name)| {
                    let pick = name.clone();
                    div()
                        .id(("cherry-target", ix as u64))
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .w_full()
                        .h(px(30.0))
                        .px(theme.space_2)
                        .rounded(theme.radius_sm)
                        .cursor_pointer()
                        .hover(move |s| s.bg(theme.surface_hover))
                        .child(icon(Icon::GitBranch, 13.0, theme.text_muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .t_body()
                                .text_color(theme.text_primary)
                                .child(name),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cherry_pick_to(pick.clone(), cx);
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .child(
                    div()
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child(format!(
                            "Apply {picked} commit{} to which branch? You will be switched to it.",
                            if picked == 1 { "" } else { "s" }
                        )),
                )
                .child(
                    div()
                        .id("cherry-target-list")
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .max_h(px(260.0))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .child(
                    button(&theme, "cherry-back", ButtonKind::Ghost, None, "Back").on_click(
                        cx.listener(|this, _, _, cx| {
                            if let Some(h) = this.history.as_mut() {
                                h.choosing_target = false;
                            }
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element()
        } else if h.loading {
            div()
                .t_small()
                .text_color(theme.text_muted)
                .child("Reading history…")
                .into_any_element()
        } else if let Some(error) = &h.error {
            div()
                .t_small()
                .text_color(theme.danger)
                .child(error.clone())
                .into_any_element()
        } else if h.plan.is_empty() {
            div()
                .t_small()
                .text_color(theme.text_muted)
                .child("Nothing to rewrite: the branch has no commits after its first one.")
                .into_any_element()
        } else {
            let last = h.plan.len() - 1;
            let rows = h
                .plan
                .iter()
                .enumerate()
                .map(|(ix, item)| self.history_row(ix, last, item, cx))
                .collect::<Vec<_>>();
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .when(h.rewording.is_some(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(self.reword_input.clone())
                            .child(
                                div()
                                    .t_micro()
                                    .text_color(theme.text_muted)
                                    .child("Enter sets the new message for that commit."),
                            ),
                    )
                })
                .child(
                    div()
                        .id("history-list")
                        .flex()
                        .flex_col()
                        .max_h(px(300.0))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .child(
                    div()
                        .t_micro()
                        .text_color(theme.text_muted)
                        .child("Newest first. Click a tag to cycle Pick, Squash, Fixup, Drop; Squash and Fixup join the commit below."),
                )
                .child(self.branch_drop_strip(repo, current.as_deref(), cx))
                .when(changed && pushed, |d| {
                    d.child(
                        div()
                            .t_small()
                            .text_color(theme.warning)
                            .child("This rewrites commits that are already pushed. You will need to force push afterwards."),
                    )
                })
                .when(dirty && changed, |d| {
                    d.child(
                        div()
                            .t_small()
                            .text_color(theme.warning)
                            .child("Commit, stash or discard your changes first: a rewrite needs a clean working copy."),
                    )
                })
                .when(in_operation, |d| {
                    d.child(
                        div()
                            .t_small()
                            .text_color(theme.warning)
                            .child("Finish or abort the operation in progress first."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .child(
                            button(
                                &theme,
                                "history-rewrite",
                                ButtonKind::Primary,
                                Some(Icon::GitCommit),
                                "Rewrite history",
                            )
                            .when(!changed || dirty || in_operation, |b| {
                                b.opacity(0.45).cursor_default()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let blocked = this.history.as_ref().is_none_or(|h| {
                                    this.pane(h.repo)
                                        .and_then(|p| p.status.as_ref())
                                        .is_none_or(|s| {
                                            !s.files.is_empty() || s.in_progress.is_some()
                                        })
                                });
                                if !blocked {
                                    this.ask_rebase(cx);
                                }
                            })),
                        )
                        .child(
                            button(
                                &theme,
                                "history-cherry",
                                ButtonKind::Secondary,
                                Some(Icon::Copy),
                                format!("Cherry-pick {picked} to…"),
                            )
                            .when(picked == 0, |b| b.opacity(0.45).cursor_default())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.history.as_ref().is_some_and(|h| !h.picked.is_empty()) {
                                    this.choose_cherry_target(cx);
                                }
                            })),
                        ),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(self.popover_title(title, cx))
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    /// Branch chips to drop commits on: a drop cherry-picks them onto that
    /// branch (and switches to it).
    fn branch_drop_strip(
        &self,
        repo: RepoId,
        current: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let branches: Vec<String> = self
            .pane(repo)
            .map(|p| p.branches.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|b| Some(b.as_str()) != current)
            .take(24)
            .collect();
        if branches.is_empty() {
            return div().into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .child(section_label(
                &theme,
                "Drag commits onto a branch to cherry-pick",
            ))
            .child(
                div()
                    .id("drop-branches")
                    .flex()
                    .flex_wrap()
                    .gap(theme.space_1)
                    .max_h(px(76.0))
                    .overflow_y_scroll()
                    .children(branches.into_iter().enumerate().map(|(ix, name)| {
                        let target = name.clone();
                        div()
                            .id(("drop-branch", ix as u64))
                            .flex()
                            .items_center()
                            .gap(theme.space_1)
                            .h(px(24.0))
                            .px(theme.space_2)
                            .rounded(theme.radius_sm)
                            .border_1()
                            .border_color(theme.border_light)
                            .t_small()
                            .text_color(theme.text_secondary)
                            .drag_over::<DraggedCommits>(move |s, _, _, _| {
                                s.bg(theme.accent.opacity(0.22))
                                    .border_color(theme.accent)
                                    .text_color(theme.text_primary)
                            })
                            .on_drop(cx.listener(
                                move |this, dragged: &DraggedCommits, _window, cx| {
                                    this.cherry_pick_hashes(
                                        target.clone(),
                                        dragged.hashes.clone(),
                                        cx,
                                    );
                                },
                            ))
                            .child(icon(Icon::GitBranch, 12.0, theme.text_muted))
                            .child(name)
                    })),
            )
            .into_any_element()
    }

    fn history_row(
        &self,
        ix: usize,
        last: usize,
        item: &PlanItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let color = action_color(&theme, &item.action);
        let dropped = item.action == PlanAction::Drop;
        let picked = self
            .history
            .as_ref()
            .is_some_and(|h| h.picked.contains(&item.hash));
        let key = ix as u64;
        let hash = item.hash.clone();
        let short: String = item.hash.chars().take(7).collect();
        // Dragging a ticked row takes every ticked commit with it; any other
        // row goes alone. Oldest first, as cherry-pick wants them.
        let dragged = {
            let take_all = picked;
            let hashes: Vec<String> = self
                .history
                .as_ref()
                .map(|h| {
                    h.plan
                        .iter()
                        .rev()
                        .filter(|p| {
                            if take_all {
                                h.picked.contains(&p.hash)
                            } else {
                                p.hash == item.hash
                            }
                        })
                        .map(|p| p.hash.clone())
                        .collect()
                })
                .unwrap_or_default();
            let label = if hashes.len() == 1 {
                item.summary.clone()
            } else {
                format!("{} commits", hashes.len())
            };
            DraggedCommits { hashes, label }
        };
        let summary = match &item.action {
            PlanAction::Reword(m) => m.clone(),
            _ => item.summary.clone(),
        };
        div()
            .id(("history-row", key))
            .flex()
            .items_center()
            .gap(theme.space_2)
            .w_full()
            .h(px(32.0))
            .px(theme.space_1)
            .border_b_1()
            .border_color(theme.border_subtle)
            .on_drag(dragged, move |d, _pos, _window, cx| {
                let label = d.label.clone();
                cx.new(|_| DragPreview { label, theme })
            })
            .child(
                checkbox(
                    &theme,
                    ("history-pick", key),
                    if picked {
                        CheckState::On
                    } else {
                        CheckState::Off
                    },
                )
                .on_click({
                    let hash = hash.clone();
                    cx.listener(move |this, _, _, cx| this.toggle_pick(hash.clone(), cx))
                }),
            )
            .child(
                div()
                    .id(("history-action", key))
                    .flex_none()
                    .w(px(58.0))
                    .cursor_pointer()
                    .tooltip(crate::ui::tip(&theme, "history-action"))
                    .on_click(cx.listener(move |this, _, _, cx| this.cycle_row(ix, cx)))
                    .child(chip(&theme, item.action.label(), color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .t_body()
                    .text_color(if dropped {
                        theme.text_muted
                    } else {
                        theme.text_primary
                    })
                    .when(dropped, |d| d.line_through())
                    .child(summary),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(CODE_FONT)
                    .t_micro()
                    .text_color(if item.unpushed {
                        theme.text_muted
                    } else {
                        theme.warning
                    })
                    .child(short),
            )
            .child(
                icon_button(
                    &theme,
                    Icon::ChevronUp,
                    ("history-up", key),
                    theme.text_primary,
                )
                .when(ix == 0, |b| b.opacity(0.35))
                .on_click(cx.listener(move |this, _, _, cx| this.move_row(ix, true, cx))),
            )
            .child(
                icon_button(
                    &theme,
                    Icon::ChevronDown,
                    ("history-down", key),
                    theme.text_primary,
                )
                .when(ix == last, |b| b.opacity(0.35))
                .on_click(cx.listener(move |this, _, _, cx| this.move_row(ix, false, cx))),
            )
            .child(
                icon_button(
                    &theme,
                    Icon::Pencil,
                    ("history-reword", key),
                    theme.text_primary,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_reword(ix, cx);
                    let focus = this.reword_input.read(cx).focus_handle.clone();
                    window.focus(&focus, cx);
                })),
            )
            .child(
                icon_button(
                    &theme,
                    Icon::ArrowLeft,
                    ("history-revert", key),
                    theme.warning,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.revert_row(hash.clone(), cx);
                })),
            )
            .into_any_element()
    }

    // ---- merge ---------------------------------------------------------

    pub(crate) fn open_merge(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        self.history = None;
        self.popover = Some(Popover::Merge);
        self.merge = Some(MergeState {
            repo,
            branch: None,
            loading: false,
            preview: None,
        });
        self.branch_filter.update(cx, |i, cx| {
            i.set_text_selected("");
            cx.notify();
        });
        self.load_branches(repo, cx);
        cx.notify();
    }

    /// Looks at what merging `branch` would do before anything changes.
    fn choose_merge_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        let Some(m) = self.merge.as_mut() else { return };
        let repo = m.repo;
        m.branch = Some(branch.clone());
        m.loading = true;
        m.preview = None;
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        self.run(
            cx,
            {
                let branch = branch.clone();
                move || git::merge_preview(&path, &branch).map_err(|e| e.to_string())
            },
            move |this, result, cx| {
                if let Some(m) = this
                    .merge
                    .as_mut()
                    .filter(|m| m.repo == repo && m.branch.as_deref() == Some(&branch))
                {
                    m.loading = false;
                    m.preview = Some(result);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn do_merge(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.merge.as_ref() else { return };
        let (repo, Some(branch)) = (m.repo, m.branch.clone()) else {
            return;
        };
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.close_popover(cx);
        if !self.begin_work(&[repo], &format!("Merging {branch}…"), cx) {
            return;
        }
        self.run(
            cx,
            {
                let branch = branch.clone();
                move || git::merge_branch(&path, &branch)
            },
            move |this, result, cx| {
                this.end_work(&[repo]);
                match result {
                    Ok(git::MergeOutcome::Merged) => this.set_notice(
                        vec![NoticeLine::ok(format!("{name}: merged {branch}"))],
                        cx,
                    ),
                    Ok(git::MergeOutcome::UpToDate) => this.set_notice(
                        vec![NoticeLine::ok(format!("{name}: already contains {branch}"))],
                        cx,
                    ),
                    Ok(git::MergeOutcome::Conflicts(n)) => this.set_notice(
                        vec![NoticeLine::warn(format!(
                            "{name}: merging {branch} stopped on {n} conflicting file{}. Resolve them in the list, then continue",
                            if n == 1 { "" } else { "s" }
                        ))],
                        cx,
                    ),
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }

    pub(super) fn merge_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(m) = self.merge.as_ref() else {
            return div().into_any_element();
        };
        let repo = m.repo;
        let current = self.current_branch_of(repo);
        let title = format!(
            "Merge into {} · {}",
            current.clone().unwrap_or_else(|| "(detached)".into()),
            self.repo_name(repo)
        );
        let query = self.branch_filter.read(cx).content.to_lowercase();
        let mut names: Vec<String> = self
            .pane(repo)
            .map(|p| p.branches.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|b| Some(b) != current.as_ref())
            .filter(|b| query.is_empty() || b.to_lowercase().contains(&query))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        let m = self.merge.as_ref().expect("checked above");
        let chosen = m.branch.clone();
        let rows = names
            .into_iter()
            .enumerate()
            .map(|(ix, name)| {
                let is_chosen = chosen.as_deref() == Some(name.as_str());
                let pick = name.clone();
                div()
                    .id(("merge-row", ix as u64))
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .w_full()
                    .h(px(30.0))
                    .px(theme.space_2)
                    .rounded(theme.radius_sm)
                    .cursor_pointer()
                    .when(is_chosen, |d| d.bg(theme.accent.opacity(0.10)))
                    .hover(move |s| s.bg(theme.surface_hover))
                    .child(icon(
                        Icon::GitBranch,
                        13.0,
                        if is_chosen {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .t_body()
                            .text_color(theme.text_primary)
                            .child(name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.choose_merge_branch(pick.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let summary: Option<AnyElement> = chosen.as_ref().map(|branch| {
            let (text, color, can_merge) = match (&m.preview, m.loading) {
                (_, true) | (None, _) => ("Checking…".to_string(), theme.text_muted, false),
                (Some(Err(e)), _) => (e.clone(), theme.danger, false),
                (Some(Ok(p)), _) if p.up_to_date => (
                    format!("{branch} is already part of this branch. Nothing to merge."),
                    theme.text_secondary,
                    false,
                ),
                (Some(Ok(p)), _) if !p.conflicts.is_empty() => (
                    format!(
                        "{} commit{} will be merged, but {} file{} conflict: {}. You can resolve them after the merge starts.",
                        p.commits,
                        if p.commits == 1 { "" } else { "s" },
                        p.conflicts.len(),
                        if p.conflicts.len() == 1 { "" } else { "s" },
                        p.conflicts.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
                    ),
                    theme.warning,
                    true,
                ),
                (Some(Ok(p)), _) => (
                    format!(
                        "{} commit{} will be merged{}. No conflicts.",
                        p.commits,
                        if p.commits == 1 { "" } else { "s" },
                        if p.fast_forward { " (fast-forward moves are recorded as a merge commit)" } else { "" }
                    ),
                    theme.success,
                    true,
                ),
            };
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .child(div().t_small().text_color(color).child(text))
                .child(
                    button(
                        &theme,
                        "merge-run",
                        ButtonKind::Primary,
                        Some(Icon::GitMerge),
                        format!("Merge {branch}"),
                    )
                    .when(!can_merge, |b| b.opacity(0.45).cursor_default())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let ready = this.merge.as_ref().is_some_and(|m| {
                            matches!(&m.preview, Some(Ok(p)) if !p.up_to_date)
                        });
                        if ready {
                            this.do_merge(cx);
                        }
                    })),
                )
                .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(self.popover_title(title, cx))
            .child(self.branch_filter.clone())
            .child(
                div()
                    .id("merge-list")
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .children(summary)
            .into_any_element()
    }

    // ---- clone ---------------------------------------------------------

    pub(super) fn clone_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(self.popover_title("Clone a repository".to_string(), cx))
            .child(section_label(&theme, "Repository address"))
            .child(self.clone_url.clone())
            .child(
                div()
                    .t_micro()
                    .text_color(theme.text_muted)
                    .child("Uses your git credentials and SSH keys. It is linked to the selected project when it finishes."),
            )
            .child(
                button(
                    &theme,
                    "clone-run",
                    ButtonKind::Primary,
                    Some(Icon::Download),
                    "Choose folder and clone",
                )
                .on_click(cx.listener(|this, _, _, cx| this.choose_clone_folder(cx))),
            )
            .into_any_element()
    }
}
