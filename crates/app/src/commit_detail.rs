//! A past commit's changes: its files on the left and the diff of the chosen
//! one on the right, opened from the history browser. Read-only; drawn like
//! the Changes view's diff (GitHub Desktop style: washed rows, tinted
//! gutters, syntax colours). All git work runs on background tasks.

use std::ops::Range;
use std::path::PathBuf;

use devspace_git::{self as git, ChangeKind, CommitChanges, FileDiff, LineKind};
use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, Context, HighlightStyle, Hsla, SharedString,
    StyledText, UniformListScrollHandle, Window,
};

use crate::changes_model::{diff_rows, DiffRow};
use crate::theme::{syntax_color, GlassTheme, TypeScale, CODE_FONT};
use crate::ui::{back_button, empty_state, notice, page_header, Icon};

const FILE_ROW_HEIGHT: f32 = 30.0;
const DIFF_ROW_HEIGHT: f32 = 22.0;
const FILES_WIDTH: f32 = 340.0;
const GUTTER_WIDTH: f32 = 44.0;

pub type CloseHandler = Box<dyn Fn(&mut Window, &mut gpui::App)>;

pub struct CommitDetail {
    theme: GlassTheme,
    repo_path: PathBuf,
    hash: String,
    title: String,
    meta: String,
    changes: Option<CommitChanges>,
    error: Option<String>,
    selected: Option<usize>,
    diff: Option<(FileDiff, Vec<DiffRow>)>,
    diff_error: Option<String>,
    diff_loading: bool,
    /// Bumped per diff request so a slow answer can't replace a newer one.
    generation: u64,
    file_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    pub on_close: Option<CloseHandler>,
}

fn kind_color(theme: &GlassTheme, kind: ChangeKind) -> Hsla {
    match kind {
        ChangeKind::Added | ChangeKind::Untracked => theme.success,
        ChangeKind::Modified | ChangeKind::Typechange => theme.warning,
        ChangeKind::Deleted | ChangeKind::Conflicted => theme.danger,
        ChangeKind::Renamed => theme.accent,
    }
}

impl CommitDetail {
    /// Opens the details of commit `hash` in the repo at `repo_path`.
    pub fn new(
        cx: &mut Context<Self>,
        theme: GlassTheme,
        repo_path: PathBuf,
        hash: String,
        title: String,
        meta: String,
    ) -> Self {
        let mut this = Self {
            theme,
            repo_path,
            hash,
            title,
            meta,
            changes: None,
            error: None,
            selected: None,
            diff: None,
            diff_error: None,
            diff_loading: false,
            generation: 0,
            file_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            on_close: None,
        };
        this.load_files(cx);
        this
    }

    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| done(this, result, cx));
        })
        .detach();
    }

    fn load_files(&mut self, cx: &mut Context<Self>) {
        let (path, hash) = (self.repo_path.clone(), self.hash.clone());
        self.run(
            cx,
            move || git::commit_changes(&path, &hash),
            |this, result, cx| {
                match result {
                    Ok(changes) => {
                        let first = (!changes.files.is_empty()).then_some(0);
                        this.changes = Some(changes);
                        if let Some(ix) = first {
                            this.select(ix, cx);
                        }
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            },
        );
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(file) = self
            .changes
            .as_ref()
            .and_then(|c| c.files.get(ix))
            .map(|f| f.path.clone())
        else {
            return;
        };
        self.selected = Some(ix);
        self.diff = None;
        self.diff_error = None;
        self.diff_loading = true;
        self.generation += 1;
        let generation = self.generation;
        let (path, hash) = (self.repo_path.clone(), self.hash.clone());
        self.run(
            cx,
            move || git::commit_file_diff(&path, &hash, &file),
            move |this, result, cx| {
                if this.generation != generation {
                    return;
                }
                this.diff_loading = false;
                match result {
                    Ok(diff) => {
                        let rows = diff_rows(&diff);
                        this.diff = Some((diff, rows));
                    }
                    Err(e) => this.diff_error = Some(e.to_string()),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn file_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(file) = self.changes.as_ref().and_then(|c| c.files.get(ix)) else {
            return div().into_any_element();
        };
        let selected = self.selected == Some(ix);
        let (dir, name) = match file.path.rfind('/') {
            Some(i) => (&file.path[..=i], &file.path[i + 1..]),
            None => ("", file.path.as_str()),
        };
        div()
            .id(("commit-file", ix as u64))
            .flex()
            .items_center()
            .gap(theme.space_2)
            .w_full()
            .h(px(FILE_ROW_HEIGHT))
            .px(theme.space_3)
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.accent.opacity(0.10)))
            .when(!selected, |d| d.hover(move |s| s.bg(theme.surface_hover)))
            .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .t_small()
                    .t_code()
                    .t_strong()
                    .text_color(kind_color(&theme, file.kind))
                    .child(file.kind.letter()),
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
            .into_any_element()
    }

    fn diff_row(&self, ix: usize) -> AnyElement {
        let theme = self.theme;
        let Some((diff, rows)) = &self.diff else {
            return div().into_any_element();
        };
        match rows.get(ix).copied() {
            Some(DiffRow::Hunk(h)) => div()
                .flex()
                .items_center()
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
                        .child(diff.hunks[h].header.clone()),
                )
                .into_any_element(),
            Some(DiffRow::Line { hunk, line }) => {
                let l = &diff.hunks[hunk].lines[line];
                let (bg, gutter_tint, marker, marker_color) = match l.kind {
                    LineKind::Added => (
                        theme.success.opacity(0.24),
                        theme.success.opacity(0.40),
                        "+",
                        theme.success,
                    ),
                    LineKind::Removed => (
                        theme.danger.opacity(0.26),
                        theme.danger.opacity(0.44),
                        "−",
                        theme.danger,
                    ),
                    _ => (
                        gpui::transparent_black(),
                        theme.surface_sunken,
                        " ",
                        theme.text_muted,
                    ),
                };
                let changed = matches!(l.kind, LineKind::Added | LineKind::Removed);
                let text: SharedString = l
                    .text
                    .replace('\t', "    ")
                    .trim_end_matches('\r')
                    .to_string()
                    .into();
                let highlights: Vec<(Range<usize>, HighlightStyle)> =
                    crate::highlight::language_for_path(&diff.path)
                        .map(|lang| {
                            crate::highlight::tokenize(lang, &text)
                                .into_iter()
                                .map(|(range, token)| {
                                    (
                                        range,
                                        HighlightStyle {
                                            color: Some(syntax_color(&theme, token)),
                                            ..Default::default()
                                        },
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                let number = |n: Option<u32>| {
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .flex_none()
                        .w(px(GUTTER_WIDTH))
                        .h_full()
                        .pr(theme.space_2)
                        .bg(gutter_tint)
                        .text_color(if changed {
                            theme.text_primary
                        } else {
                            theme.text_muted
                        })
                        .when(changed, |d| d.t_strong())
                        .child(n.map(|n| n.to_string()).unwrap_or_default())
                };
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(DIFF_ROW_HEIGHT))
                    .bg(bg)
                    .font_family(CODE_FONT)
                    .t_small()
                    .child(number(l.old_lineno))
                    .child(number(l.new_lineno))
                    .child(
                        div()
                            .flex_none()
                            .w(px(20.0))
                            .pl(theme.space_2)
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
                            .child(StyledText::new(text).with_highlights(highlights)),
                    )
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    fn render_diff(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let note = |title: &str, hint: &str| {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(empty_state(
                    &theme,
                    Icon::GitCommit,
                    title.to_string(),
                    hint.to_string(),
                    None,
                ))
                .into_any_element()
        };
        if let Some(error) = &self.diff_error {
            return div()
                .p(theme.space_4)
                .child(notice(&theme, error.clone(), true))
                .into_any_element();
        }
        let Some((diff, rows)) = &self.diff else {
            return note(
                if self.diff_loading {
                    "Loading diff…"
                } else {
                    "Select a file"
                },
                "",
            );
        };
        if diff.binary {
            return note("Binary file", "There is no text diff to show.");
        }
        if rows.is_empty() {
            return note(
                "No textual changes",
                "The file's mode or only its name changed.",
            );
        }
        let truncated = diff.truncated;
        let count = rows.len();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(
                uniform_list(
                    "commit-diff-rows",
                    count,
                    cx.processor(|this, range: Range<usize>, _window, _cx| {
                        range.map(|ix| this.diff_row(ix)).collect::<Vec<_>>()
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
                        .child("This diff is long; only the first part is shown."),
                )
            })
            .into_any_element()
    }
}

impl Render for CommitDetail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let count = self.changes.as_ref().map_or(0, |c| c.files.len());
        let truncated = self.changes.as_ref().is_some_and(|c| c.truncated);

        let left: AnyElement = if let Some(error) = &self.error {
            div()
                .p(theme.space_4)
                .child(notice(&theme, error.clone(), true))
                .into_any_element()
        } else if self.changes.is_none() {
            div()
                .p(theme.space_4)
                .t_small()
                .text_color(theme.text_muted)
                .child("Reading the commit…")
                .into_any_element()
        } else if count == 0 {
            div()
                .p(theme.space_4)
                .t_small()
                .text_color(theme.text_muted)
                .child("This commit changed no files.")
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .flex_none()
                        .px(theme.space_3)
                        .py(theme.space_2)
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child(format!(
                            "{count} changed file{}{}",
                            if count == 1 { "" } else { "s" },
                            if truncated { " (first part shown)" } else { "" }
                        )),
                )
                .child(
                    uniform_list(
                        "commit-files",
                        count,
                        cx.processor(|this, range: Range<usize>, _window, cx| {
                            range.map(|ix| this.file_row(ix, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.file_scroll)
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0)),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .min_h(px(0.0))
            .child(page_header(
                &theme,
                self.title.clone(),
                Some(self.meta.clone().into()),
                Some(
                    back_button(&theme, "commit-detail-back")
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(close) = &this.on_close {
                                close(window, cx);
                            }
                        }))
                        .into_any_element(),
                ),
                None,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(px(FILES_WIDTH))
                            .h_full()
                            .min_h(px(0.0))
                            .border_r_1()
                            .border_color(theme.border_subtle)
                            .child(left),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(self.render_diff(cx)),
                    ),
            )
    }
}
