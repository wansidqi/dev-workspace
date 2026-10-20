//! The "Stashed changes" section of the Changes view: this branch's stash,
//! listed under each repo's files like GitHub Desktop does. Its files can be
//! ticked, opened (their diff against the version they were based on),
//! restored one by one or together, and discarded.

use devspace_git::ShelfKind;
use gpui::{div, px, AnyElement, Hsla};

use super::*;
use crate::theme::{label_color, TypeScale};
use crate::ui::{button, checkbox, chip, icon, icon_button, ButtonKind, CheckState, Icon};

const ROW_HEIGHT: f32 = 32.0;

fn kind_style(theme: &GlassTheme, kind: ShelfKind) -> (&'static str, Hsla) {
    match kind {
        ShelfKind::Added => ("A", theme.success),
        ShelfKind::Modified => ("M", theme.warning),
        ShelfKind::Deleted => ("D", theme.danger),
    }
}

impl ChangesView {
    /// "Stashed changes · n" with Restore and Discard for the ticked files, or
    /// for all of them when none is ticked.
    pub(super) fn render_stash_header(&self, ri: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(pane) = self.repos.get(ri) else {
            return div().into_any_element();
        };
        let Some(shelf) = &pane.shelf else {
            return div().into_any_element();
        };
        let repo = pane.repo.id;
        let total = shelf.files.len();
        let ticked = shelf
            .files
            .iter()
            .filter(|f| self.shelf_sel.contains(&(repo, f.path.clone())))
            .count();
        let (restore_label, discard_tip) = if ticked > 0 {
            (format!("Restore {ticked}"), ticked)
        } else {
            ("Restore all".to_string(), total)
        };
        let _ = discard_tip;

        div()
            .id(("stash-head", repo.0 as u64))
            .flex()
            .items_center()
            .gap(theme.space_2)
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(theme.space_3)
            .pr(theme.space_3)
            .border_t_1()
            .border_b_1()
            .border_color(theme.border_subtle)
            .bg(theme.accent.opacity(0.05))
            .child(
                icon_button(
                    &theme,
                    if pane.shelf_collapsed {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    ("stash-collapse", repo.0 as u64),
                    theme.text_primary,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = this.pane_mut(repo) {
                        p.shelf_collapsed = !p.shelf_collapsed;
                    }
                    cx.notify();
                })),
            )
            .child(icon(Icon::Archive, 14.0, theme.accent))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .t_body()
                    .t_strong()
                    .text_color(theme.text_primary)
                    .child("Stashed"),
            )
            .child(chip(
                &theme,
                format!("{} · {}", shelf.branch, total),
                label_color(&theme, &shelf.branch),
            ))
            .child(
                button(
                    &theme,
                    ("stash-restore", repo.0 as u64),
                    ButtonKind::Secondary,
                    Some(Icon::Upload),
                    restore_label,
                )
                .h(px(24.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let files = this.stash_targets(repo);
                    this.restore_stash(repo, files, cx);
                })),
            )
            .child(
                icon_button(
                    &theme,
                    Icon::Trash,
                    ("stash-discard", repo.0 as u64),
                    theme.danger,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let files = this.stash_targets(repo);
                    this.ask_discard_stash(repo, files, cx);
                })),
            )
            .into_any_element()
    }

    pub(super) fn render_stash_row(
        &self,
        ri: usize,
        fi: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(pane) = self.repos.get(ri) else {
            return div().into_any_element();
        };
        let Some(file) = pane.shelf.as_ref().and_then(|s| s.files.get(fi)) else {
            return div().into_any_element();
        };
        let repo = pane.repo.id;
        let path = file.path.clone();
        let (letter, color) = kind_style(&theme, file.kind);
        let ticked = self.shelf_sel.contains(&(repo, path.clone()));
        let is_open = self.selected_stash.as_ref() == Some(&(repo, path.clone()));
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..=i], &path[i + 1..]),
            None => ("", path.as_str()),
        };
        let key = ((ri as u64) << 32) | fi as u64;

        div()
            .id(("stash-row", key))
            .flex()
            .items_center()
            .gap(theme.space_2)
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(theme.space_6)
            .pr(theme.space_3)
            .cursor_pointer()
            .when(is_open, |d| d.bg(theme.accent.opacity(0.10)))
            .when(!is_open, |d| d.hover(move |s| s.bg(theme.surface_hover)))
            .on_click({
                let path = path.clone();
                cx.listener(move |this, _, _, cx| this.select_stash_file(repo, path.clone(), cx))
            })
            .child(
                checkbox(
                    &theme,
                    ("stash-check", key),
                    if ticked {
                        CheckState::On
                    } else {
                        CheckState::Off
                    },
                )
                .on_click({
                    let path = path.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_stash_file(repo, path.clone(), cx);
                        cx.stop_propagation();
                    })
                }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .t_small()
                    .t_code()
                    .t_strong()
                    .text_color(color)
                    .child(letter),
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
                    .child(div().text_color(theme.text_primary).child(name.to_string())),
            )
            .child(
                icon_button(
                    &theme,
                    Icon::Upload,
                    ("stash-restore-one", key),
                    theme.accent,
                )
                .on_click({
                    let path = path.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.restore_stash(repo, vec![path.clone()], cx);
                        cx.stop_propagation();
                    })
                }),
            )
            .into_any_element()
    }

    /// The right-hand pane while a stashed file is open: its diff against the
    /// version it was based on, with Restore and Discard for that file.
    pub(super) fn render_stash_diff_panel(
        &mut self,
        repo: RepoId,
        file: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let repo_name = self.repo_name(repo);
        let kind = self
            .pane(repo)
            .and_then(|p| p.shelf.as_ref())
            .and_then(|s| s.files.iter().find(|f| f.path == file))
            .map(|f| f.kind);
        let (dir, name) = match file.rfind('/') {
            Some(i) => (file[..=i].to_string(), file[i + 1..].to_string()),
            None => (String::new(), file.clone()),
        };
        let (added, removed) = self.diff.as_ref().map_or((0, 0), |d| {
            d.diff
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .fold((0, 0), |(a, r), l| match l.kind {
                    devspace_git::LineKind::Added => (a + 1, r),
                    devspace_git::LineKind::Removed => (a, r + 1),
                    _ => (a, r),
                })
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
            .child(chip(&theme, "Stashed", theme.accent))
            .child(chip(
                &theme,
                repo_name.clone(),
                label_color(&theme, &repo_name),
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
                    .child(div().text_color(theme.text_muted).child(dir))
                    .child(div().text_color(theme.text_primary).t_strong().child(name)),
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
            );

        let bar = div()
            .flex()
            .items_center()
            .gap(theme.space_2)
            .flex_none()
            .h(px(40.0))
            .px(theme.space_4)
            .border_b_1()
            .border_color(theme.border_subtle)
            .bg(theme.accent.opacity(0.06))
            .child(
                div()
                    .flex_1()
                    .t_small()
                    .text_color(theme.text_secondary)
                    .child(match kind {
                        Some(ShelfKind::Added) => "A new file in your stash",
                        Some(ShelfKind::Deleted) => "A file deleted in your stash",
                        _ => "Changes in your stash, compared with the version they started from",
                    }),
            )
            .child({
                let file = file.clone();
                button(
                    &theme,
                    "stash-diff-restore",
                    ButtonKind::Secondary,
                    Some(Icon::Upload),
                    "Restore file",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restore_stash(repo, vec![file.clone()], cx);
                }))
            })
            .child({
                let file = file.clone();
                button(
                    &theme,
                    "stash-diff-discard",
                    ButtonKind::Danger,
                    Some(Icon::Trash),
                    "Discard",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ask_discard_stash(repo, vec![file.clone()], cx);
                }))
            });

        let body = self.render_diff_body(cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(header)
            .child(bar)
            .child(body)
            .into_any_element()
    }
}
