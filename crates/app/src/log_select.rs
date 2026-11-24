//! Text selection in the run-command log.
//!
//! The log is a virtual list, so only the visible rows exist as elements. A
//! selection therefore refers to lines by their absolute number (lines the
//! buffer already dropped plus the row's index, which stays put as the log
//! grows and scrolls off its front) and a byte offset; the rows on screen
//! register their layouts each frame for hit-testing, and copying reads the
//! text from the log itself, so a selection can reach beyond what is visible.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{actions, App, KeyBinding, Pixels, Point, TextLayout};

use crate::preview_select::{ordered, range_for, Pos};

actions!(devspace_run_log, [LogCopy, LogSelectAll]);

/// Copy and select-all while the log has focus.
pub fn init_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-c", LogCopy, Some("RunLog")),
        KeyBinding::new("cmd-c", LogCopy, Some("RunLog")),
        KeyBinding::new("ctrl-a", LogSelectAll, Some("RunLog")),
        KeyBinding::new("cmd-a", LogSelectAll, Some("RunLog")),
    ]);
}

/// A visible log row: its absolute line number, layout and text length.
struct Row {
    line: usize,
    layout: TextLayout,
    len: usize,
}

/// The log's selection, shared by the row renderer and the mouse handlers.
#[derive(Default)]
pub struct LogSelection {
    rows: Vec<Row>,
    anchor: Option<Pos>,
    head: Option<Pos>,
    dragging: bool,
}

/// Shared handle to a [`LogSelection`].
pub type Shared = Rc<RefCell<LogSelection>>;

/// The copied text: each line's covered slice, one per line.
fn assemble<'a>(lines: impl Iterator<Item = (usize, &'a str)>, start: Pos, end: Pos) -> String {
    let mut out = String::new();
    for (n, (line, text)) in lines.enumerate() {
        if n > 0 {
            out.push('\n');
        }
        if let Some(range) = range_for(start, end, line, text.len()) {
            out.push_str(text.get(range).unwrap_or(""));
        }
    }
    out
}

impl LogSelection {
    /// Forgets last frame's rows; the visible ones register again.
    pub fn begin_frame(&mut self) {
        self.rows.clear();
    }

    pub fn register(&mut self, line: usize, layout: TextLayout, len: usize) {
        self.rows.push(Row { line, layout, len });
    }

    /// The selected part of line `line` (of `len` bytes), for highlighting it.
    pub fn range_in(&self, line: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = ordered(self.anchor?, self.head?);
        range_for(start, end, line, len)
    }

    /// The selection's ends in order, or `None` when nothing is selected.
    pub fn span(&self) -> Option<(Pos, Pos)> {
        let (start, end) = ordered(self.anchor?, self.head?);
        (start != end).then_some((start, end))
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Mouse down: a new empty selection starts at the pointer.
    pub fn start(&mut self, at: Point<Pixels>) {
        self.clear();
        if let Some(pos) = self.pos_at(at) {
            self.anchor = Some(pos);
            self.head = Some(pos);
            self.dragging = true;
        }
    }

    /// Mouse move while pressed; whether the selection changed.
    pub fn drag(&mut self, at: Point<Pixels>) -> bool {
        let Some(pos) = self.pos_at(at) else {
            return false;
        };
        let changed = self.head != Some(pos);
        self.head = Some(pos);
        changed
    }

    /// Mouse up. A press that never moved selects nothing.
    pub fn finish(&mut self) {
        self.dragging = false;
        if self.anchor == self.head {
            self.clear();
        }
    }

    /// Selects `range` of `line` (a double-click's word, a triple-click's line).
    pub fn select_range(&mut self, line: usize, range: Range<usize>) {
        self.anchor = Some((line, range.start));
        self.head = Some((line, range.end));
        self.dragging = false;
    }

    /// Selects everything from the first to the last line.
    pub fn select_all(&mut self, first: usize, last: usize, last_len: usize) {
        self.anchor = Some((first, 0));
        self.head = Some((last, last_len));
        self.dragging = false;
    }

    /// The line and byte offset under the pointer, or the nearest visible one
    /// when it is outside the rows (above or below the viewport, say).
    pub fn pos_at(&self, at: Point<Pixels>) -> Option<Pos> {
        let mut best: Option<(&Row, Pixels, Pixels)> = None;
        for row in &self.rows {
            let b = row.layout.bounds();
            let dy = (b.top() - at.y).max(at.y - b.bottom()).max(Pixels::ZERO);
            let dx = (b.left() - at.x).max(at.x - b.right()).max(Pixels::ZERO);
            if best.is_none_or(|(_, by, bx)| dy < by || (dy == by && dx < bx)) {
                best = Some((row, dy, dx));
            }
        }
        let (row, ..) = best?;
        let b = row.layout.bounds();
        let inside = gpui::point(
            at.x.max(b.left()).min(b.right() - gpui::px(1.0)),
            at.y.max(b.top()).min(b.bottom() - gpui::px(1.0)),
        );
        let offset = row
            .layout
            .index_for_position(inside)
            .unwrap_or_else(|nearest| nearest);
        Some((row.line, offset.min(row.len)))
    }

    /// The selected text, read from `lines` (absolute number, text), which
    /// must be in order.
    pub fn selected_text<'a>(
        &self,
        lines: impl Iterator<Item = (usize, &'a str)>,
    ) -> Option<String> {
        let (start, end) = self.span()?;
        Some(assemble(
            lines.filter(|(line, _)| (start.0..=end.0).contains(line)),
            start,
            end,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_the_covered_part_of_each_line() {
        let log = ["alpha beta", "gamma", "delta epsilon"];
        let lines = || log.iter().enumerate().map(|(i, l)| (i + 10, *l));
        // From "beta" on line 10 to "delt" on line 12.
        let text = assemble(
            lines().filter(|(n, _)| (10..=12).contains(n)),
            (10, 6),
            (12, 4),
        );
        assert_eq!(text, "beta\ngamma\ndelt");
        // Inside one line.
        let text = assemble(lines().filter(|(n, _)| *n == 12), (12, 6), (12, 13));
        assert_eq!(text, "epsilon");
    }

    #[test]
    fn a_selection_survives_lines_being_dropped_from_the_front() {
        // Line numbers are absolute, so the same pair still picks the same
        // text after the buffer's oldest lines are gone.
        let mut sel = LogSelection::default();
        sel.select_range(42, 1..3);
        assert_eq!(sel.range_in(42, 5), Some(1..3));
        assert_eq!(sel.range_in(41, 5), None);
        assert_eq!(
            sel.selected_text([(41, "xxxxx"), (42, "abcde")].into_iter()),
            Some("bc".to_string())
        );
    }

    #[test]
    fn nothing_selected_copies_nothing() {
        let mut sel = LogSelection::default();
        assert!(sel.selected_text(std::iter::empty()).is_none());
        sel.select_range(1, 2..2);
        assert!(sel.span().is_none());
    }
}
