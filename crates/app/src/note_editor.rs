//! A reusable multiline text editor entity for note content. Adapted from
//! `text_input.rs` (itself adapted from gpui's `examples/input.rs`, see that
//! file's header comment) — the main difference is that content wraps across
//! multiple shaped lines instead of one, so cursor movement, hit-testing and
//! painting all need a line index alongside the byte offset.
//!
//! Up/Down keep the on-screen x position (the editor uses the proportional UI
//! font), and every edit or cursor move scrolls the cursor into view.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, AvailableSpace, Bounds,
    ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ScrollHandle, SharedString, Size, Style, Subscription, TextRun, TextStyle, UTF16Selection,
    Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::refs::{self, Candidate};
use crate::text_input::TextInput;
use crate::theme::{GlassTheme, TypeScale};
use crate::ui::{icon_button, Icon};

pub type ChangeHandler = Box<dyn Fn(&str, &mut Window, &mut Context<NoteEditor>)>;
pub type CloseHandler = Box<dyn Fn(&mut Window, &mut Context<NoteEditor>)>;
/// Supplies the suggestions for `[[`: the notes and tasks of the open project.
pub type RefSource = Box<dyn Fn(&App) -> Vec<Candidate>>;

actions!(
    devspace_note_editor,
    [
        NeBackspace,
        NeDelete,
        NeLeft,
        NeRight,
        NeUp,
        NeDown,
        NeSelectLeft,
        NeSelectRight,
        NeSelectAll,
        NeWordLeft,
        NeWordRight,
        NeSelectWordLeft,
        NeSelectWordRight,
        NeDeleteWordLeft,
        NeDeleteWordRight,
        NeHome,
        NeEnd,
        NeNewline,
        NePaste,
        NeCut,
        NeCopy,
        NeClose,
        NeSave,
        NeFind,
        NeFindNext,
        NeFindPrev,
        NeAcceptRef,
    ]
);

pub fn init_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", NeBackspace, Some("NoteEditor")),
        KeyBinding::new("delete", NeDelete, Some("NoteEditor")),
        KeyBinding::new("left", NeLeft, Some("NoteEditor")),
        KeyBinding::new("right", NeRight, Some("NoteEditor")),
        KeyBinding::new("up", NeUp, Some("NoteEditor")),
        KeyBinding::new("down", NeDown, Some("NoteEditor")),
        KeyBinding::new("shift-left", NeSelectLeft, Some("NoteEditor")),
        KeyBinding::new("shift-right", NeSelectRight, Some("NoteEditor")),
        KeyBinding::new("ctrl-left", NeWordLeft, Some("NoteEditor")),
        KeyBinding::new("ctrl-right", NeWordRight, Some("NoteEditor")),
        KeyBinding::new("alt-left", NeWordLeft, Some("NoteEditor")),
        KeyBinding::new("alt-right", NeWordRight, Some("NoteEditor")),
        KeyBinding::new("ctrl-shift-left", NeSelectWordLeft, Some("NoteEditor")),
        KeyBinding::new("ctrl-shift-right", NeSelectWordRight, Some("NoteEditor")),
        KeyBinding::new("alt-shift-left", NeSelectWordLeft, Some("NoteEditor")),
        KeyBinding::new("alt-shift-right", NeSelectWordRight, Some("NoteEditor")),
        KeyBinding::new("ctrl-backspace", NeDeleteWordLeft, Some("NoteEditor")),
        KeyBinding::new("ctrl-delete", NeDeleteWordRight, Some("NoteEditor")),
        KeyBinding::new("cmd-a", NeSelectAll, Some("NoteEditor")),
        KeyBinding::new("ctrl-a", NeSelectAll, Some("NoteEditor")),
        KeyBinding::new("cmd-v", NePaste, Some("NoteEditor")),
        KeyBinding::new("ctrl-v", NePaste, Some("NoteEditor")),
        KeyBinding::new("cmd-c", NeCopy, Some("NoteEditor")),
        KeyBinding::new("ctrl-c", NeCopy, Some("NoteEditor")),
        KeyBinding::new("cmd-x", NeCut, Some("NoteEditor")),
        KeyBinding::new("ctrl-x", NeCut, Some("NoteEditor")),
        KeyBinding::new("home", NeHome, Some("NoteEditor")),
        KeyBinding::new("end", NeEnd, Some("NoteEditor")),
        KeyBinding::new("enter", NeNewline, Some("NoteEditor")),
        KeyBinding::new("tab", NeAcceptRef, Some("NoteEditor")),
        KeyBinding::new("escape", NeClose, Some("NoteEditor")),
        KeyBinding::new("ctrl-s", NeSave, Some("NoteEditor")),
        KeyBinding::new("cmd-s", NeSave, Some("NoteEditor")),
        KeyBinding::new("ctrl-f", NeFind, Some("NoteEditor")),
        KeyBinding::new("cmd-f", NeFind, Some("NoteEditor")),
        KeyBinding::new("f3", NeFindNext, Some("NoteEditor")),
        KeyBinding::new("shift-f3", NeFindPrev, Some("NoteEditor")),
    ]);
}

/// What the suggestion popup shows: where the typed text starts (just after
/// `[[`, or at the `#`), the matches, and whether it is the `#` task picker.
struct RefPopup {
    start: usize,
    list: Vec<Candidate>,
    hash: bool,
}

pub struct NoteEditor {
    pub focus_handle: FocusHandle,
    pub content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// One shaped (soft-wrapped) line per `\n`-delimited row, refreshed on
    /// every paint. A line spans several visual rows when it is wrapped.
    last_lines: Vec<WrappedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// Height of one visual row at the last paint.
    last_line_height: Pixels,
    is_selecting: bool,
    /// Called after every edit with the full content, so the caller can
    /// persist it (e.g. to the note's file on disk).
    pub on_change: Option<ChangeHandler>,
    /// Called on Escape.
    pub on_close: Option<CloseHandler>,
    /// Called on Ctrl+S (the caller switches to the preview).
    pub on_save: Option<CloseHandler>,
    /// Lists the notes and tasks a `[[` can point at (set by the page that
    /// owns the project). Called only while the popup could show.
    pub ref_source: Option<RefSource>,
    /// Lists the tasks a `#` can point at; same owner and rules as `ref_source`.
    pub task_source: Option<RefSource>,
    /// The highlighted row of the `[[` popup.
    ref_selected: usize,
    /// The reference (by where its query starts) that Escape closed the popup
    /// for, so it stays closed until the caret moves on to another `[[`.
    ref_dismissed: Option<usize>,
    /// Tracks the scroller so the cursor can be kept in view while typing.
    scroll_handle: ScrollHandle,
    /// Set by every selection or content change; the next paint scrolls the
    /// cursor into view and clears it, so the mouse wheel is never fought.
    reveal_cursor: bool,
    /// The find bar (Ctrl+F): its query box, the matches of the query in the
    /// text, and which one is current. Matches are recomputed on render only
    /// when the text or the query changed (`find_key`).
    find_open: bool,
    find_input: Entity<TextInput>,
    find_matches: Vec<Range<usize>>,
    find_current: Option<usize>,
    find_key: (SharedString, SharedString),
    /// Keeps the editor repainting as the query is typed.
    _find_watch: Subscription,
    pub theme: GlassTheme,
}

impl NoteEditor {
    pub fn new(
        cx: &mut Context<Self>,
        initial_content: impl Into<SharedString>,
        theme: GlassTheme,
    ) -> Self {
        let content = initial_content.into();
        let end = content.len();
        let find_input = cx.new(|cx| TextInput::new(cx, "Find in note", theme));
        let weak = cx.weak_entity();
        find_input.update(cx, |input, _| {
            input.keep_on_confirm = true;
            let next = weak.clone();
            // Enter steps to the next match; Escape closes the bar.
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
        let find_watch = cx.observe(&find_input, |_, _, cx| cx.notify());
        Self {
            find_open: false,
            find_input,
            find_matches: Vec::new(),
            find_current: None,
            find_key: (SharedString::default(), SharedString::default()),
            _find_watch: find_watch,
            focus_handle: cx.focus_handle(),
            content,
            selected_range: end..end,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            last_line_height: px(0.0),
            is_selecting: false,
            on_change: None,
            on_close: None,
            on_save: None,
            ref_source: None,
            task_source: None,
            ref_selected: 0,
            ref_dismissed: None,
            scroll_handle: ScrollHandle::new(),
            reveal_cursor: true,
            theme,
        }
    }

    /// The `[[` suggestion popup, when it should be showing: where the typed
    /// query starts, and the matching notes and tasks. `None` while the caret
    /// is not inside an unfinished `[[`, after Escape closed it for this
    /// reference, or when nothing matches.
    fn ref_popup(&self, cx: &App) -> Option<RefPopup> {
        if !self.selected_range.is_empty() {
            return None;
        }
        let caret = self.cursor_offset();
        if let Some((start, query)) = refs::open_query(&self.content, caret) {
            let source = self.ref_source.as_ref()?;
            if self.ref_dismissed == Some(start) {
                return None;
            }
            let list = refs::filter(query, &source(cx));
            return (!list.is_empty()).then_some(RefPopup {
                start,
                list,
                hash: false,
            });
        }
        // Otherwise a `#` after other text opens the task picker.
        let (start, query) = refs::open_task_query(&self.content, caret)?;
        let source = self.task_source.as_ref()?;
        if self.ref_dismissed == Some(start) {
            return None;
        }
        let all = source(cx);
        // A whole task number needs no completing, and Enter must stay a newline.
        let typed = refs::task_number(&format!("#{query}"));
        if typed.is_some_and(|n| all.iter().any(|c| c.task_id == Some(n))) {
            return None;
        }
        let list = refs::filter_tasks(query, &all);
        (!list.is_empty()).then_some(RefPopup {
            start,
            list,
            hash: true,
        })
    }

    /// Writes the highlighted suggestion over what was typed: `[[Title]]` with
    /// the caret after the closing brackets, or `#<number>` for the task picker.
    /// Returns false when no popup is showing, so the key keeps its normal
    /// meaning.
    fn accept_ref(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(RefPopup { start, list, hash }) = self.ref_popup(cx) else {
            return false;
        };
        let Some(pick) = list.get(self.ref_selected.min(list.len() - 1)) else {
            return false;
        };
        let caret = self.cursor_offset();
        let (end, text) = if hash {
            (caret, pick.reference.clone())
        } else {
            // Typing `[[` inside text that already has `]]` must not double them.
            let end = if self.content[caret..].starts_with("]]") {
                caret + 2
            } else {
                caret
            };
            (end, format!("{}]]", pick.reference))
        };
        let range = self.range_to_utf16(&(start..end));
        self.replace_text_in_range(Some(range), &text, window, cx);
        true
    }

    /// Moves the highlight in the suggestion popup, wrapping at both ends.
    /// Returns false when no popup is showing.
    fn step_ref(&mut self, dir: isize, cx: &mut Context<Self>) -> bool {
        let Some(RefPopup { list, .. }) = self.ref_popup(cx) else {
            return false;
        };
        let n = list.len() as isize;
        self.ref_selected = (self.ref_selected as isize + dir).rem_euclid(n) as usize;
        cx.notify();
        true
    }

    /// The `[[` popup under the caret, drawn above everything (it may hang
    /// out of the scroller) and kept inside the window.
    fn render_ref_popup(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let RefPopup { list, hash, .. } = self.ref_popup(cx)?;
        let bounds = self.last_bounds?;
        let caret = self.caret_point(self.cursor_offset())?;
        let at = bounds.origin + caret + point(px(0.0), self.last_line_height + px(4.0));
        let theme = self.theme;
        let selected = self.ref_selected.min(list.len() - 1);
        let rows = list.into_iter().enumerate().map(|(ix, c)| {
            let (glyph, label) = match (c.is_task, c.parent.is_some()) {
                // The task picker writes the number, so it is what is shown.
                (true, _) if hash => (Icon::CircleCheck, c.reference.clone()),
                (true, true) => (Icon::CircleCheck, "Subtask".to_string()),
                (true, false) => (Icon::CircleCheck, "Task".to_string()),
                (false, _) => (Icon::FileText, "Note".to_string()),
            };
            div()
                .id(("ref-suggestion", ix))
                .flex()
                .items_center()
                .gap(theme.space_2)
                .px(theme.space_2)
                .py(theme.space_1)
                .rounded(theme.radius_sm)
                .cursor_pointer()
                .when(ix == selected, |d| d.bg(theme.accent.opacity(0.16)))
                .when(ix != selected, |d| {
                    d.hover(move |s| s.bg(theme.surface_hover))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        // Keep the click from moving the caret under the popup.
                        cx.stop_propagation();
                        this.ref_selected = ix;
                        this.accept_ref(window, cx);
                    }),
                )
                .child(crate::ui::icon(glyph, 14.0, theme.text_muted))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .t_body()
                                .text_color(theme.text_primary)
                                .child(c.title),
                        )
                        // A subtask says which task it sits under.
                        .children(c.parent.map(|parent| {
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .t_small()
                                .text_color(theme.text_muted)
                                .child(format!("in {parent}"))
                        })),
                )
                .child(div().t_small().text_color(theme.text_muted).child(label))
        });
        let popup = div()
            .w(px(320.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(theme.space_1)
            .rounded(theme.radius_md)
            .border_1()
            .border_color(theme.border_light)
            .bg(theme.surface_solid)
            .shadow_lg()
            .occlude()
            .children(rows);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(at)
                    .anchor(gpui::Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.0))
                    .child(popup),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }

    fn accept_ref_action(&mut self, _: &NeAcceptRef, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accept_ref(window, cx) {
            cx.propagate();
        }
    }

    fn lines(&self) -> Vec<&str> {
        self.content.split('\n').collect()
    }

    /// Byte offset where each line starts, one entry per line from `lines()`.
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        let mut offset = 0;
        for line in self
            .lines()
            .iter()
            .take(self.lines().len().saturating_sub(1))
        {
            offset += line.len() + 1; // +1 for the '\n'
            starts.push(offset);
        }
        starts
    }

    fn line_index_for_offset(&self, offset: usize) -> usize {
        let starts = self.line_starts();
        starts
            .iter()
            .rposition(|&start| start <= offset)
            .unwrap_or(0)
    }

    fn line_range(&self, line_idx: usize) -> Range<usize> {
        let starts = self.line_starts();
        let lines = self.lines();
        let start = starts[line_idx];
        let end = start + lines[line_idx].len();
        start..end
    }

    fn left(&mut self, _: &NeLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &NeRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    fn up(&mut self, _: &NeUp, window: &mut Window, cx: &mut Context<Self>) {
        if !self.step_ref(-1, cx) {
            self.move_vertical(-1, window, cx);
        }
    }

    fn down(&mut self, _: &NeDown, window: &mut Window, cx: &mut Context<Self>) {
        if !self.step_ref(1, cx) {
            self.move_vertical(1, window, cx);
        }
    }

    /// Moves the caret one visual row up (`-1`) or down (`1`), keeping its
    /// on-screen x so wrapped lines and the proportional note font behave.
    /// Before the first paint there is no layout, so it steps by source line.
    fn move_vertical(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.cursor_offset();
        let lh = self.last_line_height;
        if let (Some(caret), true) = (self.caret_point(offset), lh > px(0.0)) {
            let y = caret.y + lh * (dir as f32) + lh * 0.5;
            let total: usize = self.last_lines.iter().map(row_count).sum();
            let target = if y < px(0.0) {
                0
            } else if y >= lh * (total as f32) {
                self.content.len()
            } else {
                self.offset_at(point(caret.x, y))
            };
            self.move_to(target, window, cx);
            return;
        }
        let line_idx = self.line_index_for_offset(offset);
        let target_line = if dir < 0 {
            if line_idx == 0 {
                self.move_to(0, window, cx);
                return;
            }
            line_idx - 1
        } else {
            if line_idx + 1 >= self.lines().len() {
                self.move_to(self.content.len(), window, cx);
                return;
            }
            line_idx + 1
        };
        let col = offset - self.line_range(line_idx).start;
        let target = self.line_range(target_line);
        self.move_to((target.start + col).min(target.end), window, cx);
    }

    /// Caret position for `offset` relative to the text origin, from the last
    /// paint's layout.
    fn caret_point(&self, offset: usize) -> Option<Point<Pixels>> {
        let idx = self.line_index_for_offset(offset);
        let line = self.last_lines.get(idx)?;
        let col = offset - self.line_starts()[idx];
        let (row, x) = caret_in_line(line, col);
        let rows: usize = self.last_lines.iter().take(idx).map(row_count).sum();
        Some(point(x, self.last_line_height * ((rows + row) as f32)))
    }

    /// Byte offset nearest `p`, a point relative to the text origin, using the
    /// last paint's layout. Below everything it is the end of the text.
    fn offset_at(&self, p: Point<Pixels>) -> usize {
        let lh = self.last_line_height;
        if self.last_lines.is_empty() || lh <= px(0.0) {
            return 0;
        }
        let mut row = (p.y.max(px(0.0)) / lh) as usize;
        let starts = self.line_starts();
        for (idx, line) in self.last_lines.iter().enumerate() {
            let n = row_count(line);
            if row < n {
                let in_line = point(p.x, lh * (row as f32) + lh * 0.5);
                let col = match line.closest_index_for_position(in_line, lh) {
                    Ok(i) | Err(i) => i,
                };
                return starts[idx] + col.min(line.len());
            }
            row -= n;
        }
        self.content.len()
    }

    fn select_left(&mut self, _: &NeSelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &NeSelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn word_left(&mut self, _: &NeWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        let from = if self.selected_range.is_empty() {
            self.cursor_offset()
        } else {
            self.selected_range.start
        };
        let to = crate::text_nav::word_start_before(&self.content, from);
        self.move_to(to, window, cx);
    }

    fn word_right(&mut self, _: &NeWordRight, window: &mut Window, cx: &mut Context<Self>) {
        let from = if self.selected_range.is_empty() {
            self.cursor_offset()
        } else {
            self.selected_range.end
        };
        let to = crate::text_nav::word_end_after(&self.content, from);
        self.move_to(to, window, cx);
    }

    fn select_word_left(
        &mut self,
        _: &NeSelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = crate::text_nav::word_start_before(&self.content, self.cursor_offset());
        self.select_to(to, window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &NeSelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = crate::text_nav::word_end_after(&self.content, self.cursor_offset());
        self.select_to(to, window, cx);
    }

    fn delete_word_left(
        &mut self,
        _: &NeDeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let to = crate::text_nav::word_start_before(&self.content, self.cursor_offset());
            if to == self.cursor_offset() {
                return;
            }
            self.select_to(to, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_right(
        &mut self,
        _: &NeDeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let to = crate::text_nav::word_end_after(&self.content, self.cursor_offset());
            if to == self.cursor_offset() {
                return;
            }
            self.select_to(to, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn select_all(&mut self, _: &NeSelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    fn home(&mut self, _: &NeHome, window: &mut Window, cx: &mut Context<Self>) {
        let line_idx = self.line_index_for_offset(self.cursor_offset());
        self.move_to(self.line_range(line_idx).start, window, cx);
    }

    fn end(&mut self, _: &NeEnd, window: &mut Window, cx: &mut Context<Self>) {
        let line_idx = self.line_index_for_offset(self.cursor_offset());
        self.move_to(self.line_range(line_idx).end, window, cx);
    }

    fn backspace(&mut self, _: &NeBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &NeDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn newline(&mut self, _: &NeNewline, window: &mut Window, cx: &mut Context<Self>) {
        if self.accept_ref(window, cx) {
            return;
        }
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn close(&mut self, _: &NeClose, window: &mut Window, cx: &mut Context<Self>) {
        // Escape first closes the suggestion popup; the next one leaves the note.
        if let Some(RefPopup { start, .. }) = self.ref_popup(cx) {
            self.ref_dismissed = Some(start);
            cx.notify();
            return;
        }
        if let Some(handler) = self.on_close.take() {
            handler(window, cx);
            self.on_close = Some(handler);
        }
    }

    fn save(&mut self, _: &NeSave, window: &mut Window, cx: &mut Context<Self>) {
        // Content is already persisted on every edit; save re-writes it so the
        // file is current even if an earlier write failed, then lets the
        // caller react (switch to preview).
        self.notify_change(window, cx);
        if let Some(handler) = self.on_save.take() {
            handler(window, cx);
            self.on_save = Some(handler);
        }
    }

    /// The editor's scroller, so a split view can scroll the preview with it.
    pub fn scroll_handle(&self) -> ScrollHandle {
        self.scroll_handle.clone()
    }

    /// Ticks or unticks the task marker at `range` (a click on a preview
    /// checkbox) and persists the note like any other edit. Ignored if the text
    /// no longer matches what the preview saw.
    pub fn toggle_task(
        &mut self,
        range: &std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((replace, text)) = crate::markdown::toggle_marker(&self.content, range) else {
            return;
        };
        let mut content = self.content.to_string();
        content.replace_range(replace, text);
        self.content = content.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.notify_change(window, cx);
        cx.notify();
    }

    /// Applies a toolbar formatting command to the current selection.
    pub fn apply_format(
        &mut self,
        format: crate::note_format::Format,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = crate::note_format::apply(&self.content, self.selected_range.clone(), format);
        let mut text = self.content.to_string();
        text.replace_range(edit.range, &edit.text);
        self.content = text.into();
        self.selected_range = edit.select;
        self.selection_reversed = false;
        self.reveal_cursor = true;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        self.notify_change(window, cx);
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            // Triple-click selects the whole line.
            let range = self.line_range(self.line_index_for_offset(offset));
            self.select_range(range, window, cx);
            return;
        }
        if event.click_count == 2 {
            // Double-click selects the word under the pointer.
            let range = crate::text_nav::word_at(&self.content, offset);
            self.select_range(range, window, cx);
            return;
        }
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), window, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        }
    }

    fn paste(&mut self, _: &NePaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &NeCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &NeCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.reveal_cursor = true;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    // ---- find ----------------------------------------------------------

    /// Ctrl+F: opens the find bar (seeded with a one-line selection) and moves
    /// focus into its query box.
    fn open_find(&mut self, _: &NeFind, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        let selected = &self.selected_range;
        if !selected.is_empty() {
            let text = &self.content[selected.clone()];
            if !text.contains('\n') && text.len() <= 120 {
                let text = text.to_string();
                self.find_input.update(cx, |input, cx| {
                    input.set_text_selected(text);
                    cx.notify();
                });
            }
        }
        let focus = self.find_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes the bar and hands focus back to the text.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        self.find_matches.clear();
        self.find_current = None;
        self.find_key = (SharedString::default(), SharedString::default());
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Recomputes the matches if the text or the query changed since last time.
    /// A new query jumps to the first match at or after the cursor.
    fn refresh_find(&mut self, cx: &Context<Self>) {
        if !self.find_open {
            return;
        }
        let query = self.find_input.read(cx).content.clone();
        if self.find_key.0 == self.content && self.find_key.1 == query {
            return;
        }
        let query_changed = self.find_key.1 != query;
        self.find_key = (self.content.clone(), query.clone());
        self.find_matches = crate::text_nav::find_all(&self.content, &query);
        if self.find_matches.is_empty() {
            self.find_current = None;
        } else if query_changed {
            let from = self.selected_range.start;
            let ix = self
                .find_matches
                .iter()
                .position(|m| m.start >= from)
                .unwrap_or(0);
            self.select_match(ix);
        } else {
            let last = self.find_matches.len() - 1;
            self.find_current = Some(self.find_current.unwrap_or(0).min(last));
        }
    }

    fn select_match(&mut self, index: usize) {
        self.find_current = Some(index);
        self.selected_range = self.find_matches[index].clone();
        self.selection_reversed = false;
        self.reveal_cursor = true;
    }

    /// Moves to the next (`+1`) or previous (`-1`) match, wrapping around.
    fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.refresh_find(cx);
        let count = self.find_matches.len();
        if count == 0 {
            return;
        }
        let from = self.find_current.unwrap_or(0) as isize;
        self.select_match((from + delta).rem_euclid(count as isize) as usize);
        cx.notify();
    }

    fn find_next(&mut self, _: &NeFindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(1, cx);
    }

    fn find_prev(&mut self, _: &NeFindPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(-1, cx);
    }

    fn render_find_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let status = match (
            self.find_matches.len(),
            self.find_input.read(cx).content.is_empty(),
        ) {
            (_, true) => String::new(),
            (0, false) => "No results".to_string(),
            (n, false) => format!("{} of {n}", self.find_current.map_or(1, |c| c + 1)),
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space_2)
            .pb(theme.space_2)
            .child(div().w(px(240.0)).child(self.find_input.clone()))
            .child(
                div()
                    .min_w(px(64.0))
                    .t_small()
                    .text_color(theme.text_muted)
                    .child(status),
            )
            .child(
                icon_button(&theme, Icon::ChevronUp, "find-prev", theme.text_primary)
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(-1, cx))),
            )
            .child(
                icon_button(&theme, Icon::ChevronDown, "find-next", theme.text_primary)
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(1, cx))),
            )
            .child(
                icon_button(&theme, Icon::X, "find-close", theme.text_primary)
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
    }

    /// Puts the cursor at byte `offset` (clamped to the text and to a character
    /// boundary) and scrolls it into view, e.g. after a click in the preview.
    pub fn place_cursor(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        self.move_to(offset, window, cx);
    }

    /// Selects `range` (a double- or triple-click) and stops any drag-select.
    fn select_range(
        &mut self,
        range: std::ops::Range<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = false;
        self.selected_range = range;
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Finds the byte offset nearest `position` using the last paint's
    /// shaped lines. Falls back to the start/end of content when the
    /// position is above/below everything that was laid out.
    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        self.offset_at(point(position.x - bounds.left(), position.y - bounds.top()))
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.reveal_cursor = true;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn notify_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The suggestion list changes with every edit, so start at its top.
        self.ref_selected = 0;
        if let Some(handler) = self.on_change.take() {
            handler(&self.content.clone(), window, cx);
            self.on_change = Some(handler);
        }
    }
}

impl EntityInputHandler for NoteEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        self.reveal_cursor = true;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.reveal_cursor = true;
        self.marked_range.take();
        window.notify_text_input_changed(&self.focus_handle);
        self.notify_change(window, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.reveal_cursor = true;

        window.notify_text_input_changed(&self.focus_handle);
        self.notify_change(window, cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.caret_point(range.start)?;
        let end = self.caret_point(range.end).unwrap_or(start);
        let right = if end.y == start.y { end.x } else { start.x };
        Some(Bounds::from_corners(
            bounds.origin + start,
            point(
                bounds.left() + right,
                bounds.top() + start.y + window.line_height(),
            ),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line_point = bounds.localize(&point)?;
        Some(self.offset_to_utf16(self.index_for_mouse_position(bounds.origin + line_point)))
    }
}

struct NoteTextElement {
    input: Entity<NoteEditor>,
    theme: GlassTheme,
}

/// Visual rows a wrapped line occupies (one more than its wrap boundaries).
fn row_count(line: &WrappedLine) -> usize {
    line.wrap_boundaries().len() + 1
}

/// Byte index at which each visual row of a wrapped line starts.
fn row_starts(line: &WrappedLine) -> Vec<usize> {
    let mut starts = vec![0];
    for b in line.wrap_boundaries() {
        if let Some(glyph) = line
            .runs()
            .get(b.run_ix)
            .and_then(|r| r.glyphs.get(b.glyph_ix))
        {
            starts.push(glyph.index);
        }
    }
    starts
}

/// The visual row holding byte `col` and the x of `col` within that row. A
/// caret on a wrap boundary shows at the start of the lower row.
fn caret_in_line(line: &WrappedLine, col: usize) -> (usize, Pixels) {
    let starts = row_starts(line);
    let row = starts.partition_point(|s| *s <= col).saturating_sub(1);
    let layout = &line.unwrapped_layout;
    let x = layout.x_for_index(col.min(line.len())) - layout.x_for_index(starts[row]);
    (row, x)
}

/// One rectangle per visual row covered by `start..end` (bytes of the line),
/// with `origin` the top-left of the line.
fn range_rects(
    line: &WrappedLine,
    start: usize,
    end: usize,
    origin: Point<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    let starts = row_starts(line);
    let layout = &line.unwrapped_layout;
    let mut rects = Vec::new();
    for (r, &row_start) in starts.iter().enumerate() {
        let row_end = starts.get(r + 1).copied().unwrap_or(line.len());
        let (a, b) = (start.max(row_start), end.min(row_end));
        if a >= b {
            continue;
        }
        let base = layout.x_for_index(row_start);
        let top = origin.y + line_height * (r as f32);
        rects.push(Bounds::from_corners(
            point(origin.x + layout.x_for_index(a) - base, top),
            point(origin.x + layout.x_for_index(b) - base, top + line_height),
        ));
    }
    rects
}

/// Shapes every source line, wrapped to `wrap` when given. Shaping is cached
/// by the text system, so measuring and painting the same text is cheap.
fn shape_wrapped(
    window: &Window,
    style: &TextStyle,
    font_size: Pixels,
    color: Hsla,
    lines: &[&str],
    wrap: Option<Pixels>,
) -> Vec<WrappedLine> {
    lines
        .iter()
        .map(|text| {
            let runs = if text.is_empty() {
                vec![]
            } else {
                vec![TextRun {
                    len: text.len(),
                    font: style.font(),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }]
            };
            window
                .text_system()
                .shape_text(
                    SharedString::from(text.to_string()),
                    font_size,
                    &runs,
                    wrap,
                    None,
                )
                .ok()
                .and_then(|mut shaped| shaped.drain(..).next())
                .unwrap_or_default()
        })
        .collect()
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    /// Find-bar matches, painted under the selection.
    finds: Vec<PaintQuad>,
}

impl IntoElement for NoteTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for NoteTextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // The height depends on how many rows the text wraps to at the width
        // the layout engine hands us, so it is measured, not fixed.
        let content = self.input.read(cx).content.clone();
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let color = self.theme.text_primary;
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let lines: Vec<&str> = content.split('\n').collect();
            let shaped = shape_wrapped(window, &text_style, font_size, color, &lines, width);
            let rows: usize = shaped.iter().map(row_count).sum();
            Size {
                width: width.unwrap_or(px(0.0)),
                height: line_height * (rows.max(1) as f32),
            }
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let lines_text: Vec<String> = input.lines().iter().map(|s| s.to_string()).collect();
        let line_starts = input.line_starts();
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let find_matches = input.find_matches.clone();
        let find_current = input.find_current;
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();

        let line_refs: Vec<&str> = lines_text.iter().map(String::as_str).collect();
        let shaped = shape_wrapped(
            window,
            &style,
            font_size,
            self.theme.text_primary,
            &line_refs,
            Some(bounds.size.width),
        );

        // Top of each source line, relative to the text origin: a wrapped
        // line is several rows tall.
        let mut tops: Vec<Pixels> = Vec::with_capacity(shaped.len());
        let mut rows_so_far = 0usize;
        for line in &shaped {
            tops.push(line_height * (rows_so_far as f32));
            rows_so_far += row_count(line);
        }

        let cursor_line_idx = input.line_index_for_offset(cursor_offset);
        let cursor_col = cursor_offset - line_starts[cursor_line_idx];
        let (cursor_row, cursor_x) = shaped
            .get(cursor_line_idx)
            .map(|l| caret_in_line(l, cursor_col))
            .unwrap_or((0, px(0.0)));
        let cursor_top = bounds.top()
            + tops.get(cursor_line_idx).copied().unwrap_or(px(0.0))
            + line_height * (cursor_row as f32);

        // Keep the cursor inside the scroller after typing or moving it. The
        // offset is read from this frame's layout, so a stale one-frame-old
        // measurement cannot cause a wrong jump; a refresh repaints at the new
        // offset.
        let (reveal, scroll_handle) = (input.reveal_cursor, input.scroll_handle.clone());
        if reveal {
            let viewport = scroll_handle.bounds();
            let margin = line_height;
            let mut offset = scroll_handle.offset();
            let cursor_bottom = cursor_top + line_height;
            if cursor_top < viewport.top() + margin {
                offset.y = (offset.y + (viewport.top() + margin - cursor_top)).min(px(0.0));
            } else if cursor_bottom > viewport.bottom() - margin {
                offset.y += viewport.bottom() - margin - cursor_bottom;
            }
            if offset != scroll_handle.offset() && viewport.size.height > px(0.0) {
                scroll_handle.set_offset(offset);
                window.refresh();
            }
            self.input
                .update(cx, |input, _| input.reveal_cursor = false);
        }

        // Only matches on screen are drawn: a short query can match thousands
        // of times.
        let viewport = scroll_handle.bounds();
        let mut finds = Vec::new();
        for (n, m) in find_matches.iter().enumerate() {
            let idx = line_starts
                .partition_point(|start| *start <= m.start)
                .saturating_sub(1);
            let Some(line) = shaped.get(idx) else {
                continue;
            };
            let top = bounds.top() + tops[idx];
            let height = line_height * (row_count(line) as f32);
            if viewport.size.height > px(0.0)
                && (top + height < viewport.top() || top > viewport.bottom())
            {
                continue;
            }
            let start = m.start - line_starts[idx];
            let end = (m.end - line_starts[idx]).min(line.len());
            let color = if find_current == Some(n) {
                self.theme.warning.opacity(0.55)
            } else {
                self.theme.warning.opacity(0.25)
            };
            for rect in range_rects(line, start, end, point(bounds.left(), top), line_height) {
                finds.push(fill(rect, color));
            }
        }

        let mut selections = Vec::new();
        if !selected_range.is_empty() {
            for (idx, line) in shaped.iter().enumerate() {
                let line_range = line_starts[idx]..line_starts[idx] + line.len();
                let overlap_start = selected_range.start.max(line_range.start);
                let overlap_end = selected_range.end.min(line_range.end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let top = bounds.top() + tops[idx];
                for rect in range_rects(
                    line,
                    overlap_start - line_range.start,
                    overlap_end - line_range.start,
                    point(bounds.left(), top),
                    line_height,
                ) {
                    selections.push(fill(rect, self.theme.accent.opacity(0.25)));
                }
            }
        }

        let cursor = selected_range.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(bounds.left() + cursor_x, cursor_top),
                    size(px(2.), line_height),
                ),
                self.theme.accent,
            )
        });

        PrepaintState {
            lines: shaped,
            cursor,
            selections,
            finds,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for find in prepaint.finds.drain(..) {
            window.paint_quad(find);
        }
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        let line_height = window.line_height();
        let mut top = bounds.top();
        for line in prepaint.lines.iter() {
            line.paint(
                point(bounds.left(), top),
                line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();
            top += line_height * (row_count(line) as f32);
        }
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }
        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _cx| {
            input.last_lines = lines;
            input.last_bounds = Some(bounds);
            input.last_line_height = line_height;
        });
    }
}

impl Render for NoteEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        self.refresh_find(cx);
        let focused = self.focus_handle.is_focused(window);
        let popup = if focused {
            self.render_ref_popup(cx)
        } else {
            None
        };
        // The scroll container carries the frame; the padded text box inside it
        // grows with the content (padding on the scroller itself clips the text).
        let editor = div()
            .flex()
            .key_context("NoteEditor")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::accept_ref_action))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .min_h_full()
            .p(theme.space_4)
            .font_family(crate::theme::CODE_FONT)
            .text_size(px(crate::theme::FS_BODY))
            .line_height(gpui::relative(crate::markdown_view::NOTE_LINE_HEIGHT))
            .child(NoteTextElement {
                input: cx.entity(),
                theme,
            })
            .children(popup);
        let scroller = div()
            .id("note-editor-scroll")
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .rounded(theme.radius_md)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(if focused {
                theme.accent.opacity(0.6)
            } else {
                theme.border_light
            })
            .child(editor);
        let bar = self.find_open.then(|| self.render_find_bar(cx));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .h_full()
            .min_h(px(0.0))
            .children(bar)
            .child(scroller)
    }
}

impl Focusable for NoteEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
