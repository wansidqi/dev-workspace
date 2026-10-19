//! A reusable multiline text editor entity for note content. Adapted from
//! `text_input.rs` (itself adapted from gpui's `examples/input.rs`, see that
//! file's header comment) — the main difference is that content wraps across
//! multiple shaped lines instead of one, so cursor movement, hit-testing and
//! painting all need a line index alongside the byte offset.
//!
//! Known limitation: Up/Down move the cursor by column measured in bytes,
//! not by on-screen x position, so it will drift on non-monospace fonts with
//! mixed-width lines. Fine for the plain-text notes this backs; revisit if a
//! proportional-font editor ever needs pixel-accurate vertical movement.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::GlassTheme;

pub type ChangeHandler = Box<dyn Fn(&str, &mut Window, &mut Context<NoteEditor>)>;
pub type CloseHandler = Box<dyn Fn(&mut Window, &mut Context<NoteEditor>)>;

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
        NeHome,
        NeEnd,
        NeNewline,
        NePaste,
        NeCut,
        NeCopy,
        NeClose,
        NeSave,
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
        KeyBinding::new("escape", NeClose, Some("NoteEditor")),
        KeyBinding::new("ctrl-s", NeSave, Some("NoteEditor")),
        KeyBinding::new("cmd-s", NeSave, Some("NoteEditor")),
    ]);
}

pub struct NoteEditor {
    pub focus_handle: FocusHandle,
    pub content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// One shaped line per `\n`-delimited row, refreshed on every paint.
    last_lines: Vec<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// Called after every edit with the full content, so the caller can
    /// persist it (e.g. to the note's file on disk).
    pub on_change: Option<ChangeHandler>,
    /// Called on Escape.
    pub on_close: Option<CloseHandler>,
    /// Called on Ctrl+S (the caller switches to the preview).
    pub on_save: Option<CloseHandler>,
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
        Self {
            focus_handle: cx.focus_handle(),
            content,
            selected_range: end..end,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            is_selecting: false,
            on_change: None,
            on_close: None,
            on_save: None,
            theme,
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
        let offset = self.cursor_offset();
        let line_idx = self.line_index_for_offset(offset);
        if line_idx == 0 {
            self.move_to(0, window, cx);
            return;
        }
        let col = offset - self.line_range(line_idx).start;
        let prev_range = self.line_range(line_idx - 1);
        let target = (prev_range.start + col).min(prev_range.end);
        self.move_to(target, window, cx);
    }

    fn down(&mut self, _: &NeDown, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.cursor_offset();
        let line_idx = self.line_index_for_offset(offset);
        let line_count = self.lines().len();
        if line_idx + 1 >= line_count {
            self.move_to(self.content.len(), window, cx);
            return;
        }
        let col = offset - self.line_range(line_idx).start;
        let next_range = self.line_range(line_idx + 1);
        let target = (next_range.start + col).min(next_range.end);
        self.move_to(target, window, cx);
    }

    fn select_left(&mut self, _: &NeSelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &NeSelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
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
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn close(&mut self, _: &NeClose, window: &mut Window, cx: &mut Context<Self>) {
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
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
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
        if self.last_lines.is_empty() {
            return 0;
        }
        let line_height = bounds.size.height / (self.last_lines.len() as f32).max(1.0);
        let relative_y = (position.y - bounds.top()).max(px(0.0));
        let line_idx = ((relative_y / line_height) as usize).min(self.last_lines.len() - 1);
        let line_start = self.line_starts()[line_idx];
        line_start + self.last_lines[line_idx].closest_index_for_x(position.x - bounds.left())
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
        let line_idx = self.line_index_for_offset(range.start);
        let line = self.last_lines.get(line_idx)?;
        let line_start = self.line_starts().get(line_idx).copied()?;
        let line_height = window.line_height();
        let top = bounds.top() + line_height * (line_idx as f32);
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(range.start - line_start),
                top,
            ),
            point(
                bounds.left() + line.x_for_index((range.end - line_start).min(line.len())),
                top + line_height,
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

struct PrepaintState {
    lines: Vec<ShapedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
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
        let line_count = self.input.read(cx).lines().len().max(1);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * (line_count as f32)).into();
        (window.request_layout(style, [], cx), ())
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
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();

        let shaped: Vec<ShapedLine> = lines_text
            .iter()
            .map(|text| {
                let run = TextRun {
                    len: text.len(),
                    font: style.font(),
                    color: self.theme.text_primary,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let runs = if text.is_empty() { vec![] } else { vec![run] };
                window.text_system().shape_line(
                    SharedString::from(text.clone()),
                    font_size,
                    &runs,
                    None,
                )
            })
            .collect();

        let cursor_line_idx = input.line_index_for_offset(cursor_offset);
        let cursor_col = cursor_offset - line_starts[cursor_line_idx];
        let cursor_top = bounds.top() + line_height * (cursor_line_idx as f32);
        let cursor_x = shaped
            .get(cursor_line_idx)
            .map(|l| l.x_for_index(cursor_col))
            .unwrap_or(px(0.0));

        let mut selections = Vec::new();
        if !selected_range.is_empty() {
            for (idx, line) in shaped.iter().enumerate() {
                let line_range = line_starts[idx]..line_starts[idx] + line.len;
                let overlap_start = selected_range.start.max(line_range.start);
                let overlap_end = selected_range.end.min(line_range.end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let top = bounds.top() + line_height * (idx as f32);
                selections.push(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(overlap_start - line_range.start),
                            top,
                        ),
                        point(
                            bounds.left() + line.x_for_index(overlap_end - line_range.start),
                            top + line_height,
                        ),
                    ),
                    self.theme.accent.opacity(0.25),
                ));
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
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        let line_height = window.line_height();
        for (idx, line) in prepaint.lines.iter().enumerate() {
            let top = bounds.top() + line_height * (idx as f32);
            line.paint(
                point(bounds.left(), top),
                line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();
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
        });
    }
}

impl Render for NoteEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        div()
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
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::save))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .flex_1()
            .w_full()
            .h_full()
            .p(theme.space_3)
            .rounded(theme.radius_md)
            .bg(theme.surface_solid)
            .border_1()
            .border_color(if self.focus_handle.is_focused(window) {
                theme.accent
            } else {
                theme.border_subtle
            })
            .child(NoteTextElement {
                input: cx.entity(),
                theme,
            })
    }
}

impl Focusable for NoteEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
