//! Text selection in the rendered note preview.
//!
//! GPUI draws each paragraph, heading, table cell and code line as its own text
//! element and has no selection across them, so the preview keeps its own
//! model: every text registers itself, in document order, each frame; a
//! selection is two positions `(text index, byte offset)`; the texts draw the
//! slice that falls inside it (see `markdown_view`), and Ctrl+C joins the
//! selected slices.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{actions, App, KeyBinding, Pixels, Point, TextLayout};

actions!(
    devspace_preview,
    [
        PreviewCopy,
        PreviewSelectAll,
        PreviewFind,
        PreviewFindNext,
        PreviewFindPrev
    ]
);

/// Copy and select-all while the preview has focus.
pub fn init_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-c", PreviewCopy, Some("NotePreview")),
        KeyBinding::new("cmd-c", PreviewCopy, Some("NotePreview")),
        KeyBinding::new("ctrl-a", PreviewSelectAll, Some("NotePreview")),
        KeyBinding::new("cmd-a", PreviewSelectAll, Some("NotePreview")),
        KeyBinding::new("ctrl-f", PreviewFind, Some("NotePreview")),
        KeyBinding::new("cmd-f", PreviewFind, Some("NotePreview")),
        KeyBinding::new("f3", PreviewFindNext, Some("NotePreview")),
        KeyBinding::new("shift-f3", PreviewFindPrev, Some("NotePreview")),
    ]);
}

/// A place in the preview: which text, and a byte offset into it.
pub type Pos = (usize, usize);

/// One selectable text, as laid out on the last paint.
pub struct Entry {
    pub layout: TextLayout,
    /// What goes between the previous text and this one when copying.
    pub join: &'static str,
    /// An empty code line (shown as a single space so it keeps its height).
    pub blank: bool,
    /// Where the find query occurs in this text (byte ranges).
    pub hits: Vec<Range<usize>>,
}

/// The preview's selection, shared between the render (which registers texts
/// and reads the range) and the mouse handlers (which move it).
#[derive(Default)]
pub struct Selection {
    entries: Vec<Entry>,
    anchor: Option<Pos>,
    head: Option<Pos>,
    dragging: bool,
    /// The find bar's query (empty: not searching), which match is current
    /// (counted across all texts), how many matches the last frame found, and
    /// whether the next paint should scroll the current one into view.
    query: String,
    current: usize,
    hit_count: usize,
    reveal: bool,
}

/// Shared handle, as the preview's owner and its mouse handlers all need it.
pub type Shared = Rc<RefCell<Selection>>;

/// The two ends of a selection in document order.
pub(crate) fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// `current` moved by `delta` among `total` matches, wrapping around.
fn wrap_step(current: usize, total: usize, delta: isize) -> usize {
    if total == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(total as isize) as usize
}

/// The part of text `ix` (of `len` bytes) that a selection from `start` to
/// `end` covers. `None` when it covers none of it.
pub(crate) fn range_for(start: Pos, end: Pos, ix: usize, len: usize) -> Option<Range<usize>> {
    if ix < start.0 || ix > end.0 {
        return None;
    }
    let from = if ix == start.0 { start.1.min(len) } else { 0 };
    let to = if ix == end.0 { end.1.min(len) } else { len };
    (from < to).then_some(from..to)
}

/// The copied text: the covered slice of each text, joined by each one's
/// separator. `parts` holds `(text, separator)` for texts `first..`.
fn assemble(parts: &[(String, &'static str)], first: usize, start: Pos, end: Pos) -> String {
    let mut out = String::new();
    for (offset, (text, join)) in parts.iter().enumerate() {
        let ix = first + offset;
        if offset > 0 {
            out.push_str(join);
        }
        if let Some(range) = range_for(start, end, ix, text.len()) {
            out.push_str(text.get(range).unwrap_or(""));
        }
    }
    out
}

impl Selection {
    /// Forgets last frame's texts; the render registers them again in order.
    pub fn begin_frame(&mut self) {
        self.entries.clear();
        self.hit_count = 0;
    }

    /// The index the next registered text will get.
    pub fn next_index(&self) -> usize {
        self.entries.len()
    }

    pub fn register(&mut self, entry: Entry) {
        self.hit_count += entry.hits.len();
        self.entries.push(entry);
    }

    // ---- find ------------------------------------------------------------

    /// Sets the text being searched for. A new query starts at its first
    /// match and asks for it to be scrolled into view.
    pub fn set_query(&mut self, query: &str) {
        if self.query != query {
            self.query = query.to_owned();
            self.current = 0;
            self.reveal = !query.is_empty();
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// How many matches the last frame found, across all texts.
    pub fn hit_total(&self) -> usize {
        self.hit_count
    }

    /// Which match is current (0-based). Only meaningful with matches.
    pub fn current_hit(&self) -> usize {
        self.current
    }

    /// How many matches were registered before the next text, so a text can
    /// tell which of its own matches is the current one.
    pub fn hits_before_next(&self) -> usize {
        self.hit_count
    }

    /// Moves to the next (+1) or previous (-1) match, wrapping around, and
    /// asks for it to be scrolled into view.
    pub fn step_hit(&mut self, delta: isize) {
        self.current = wrap_step(self.current, self.hit_count, delta);
        self.reveal = self.hit_count > 0;
    }

    /// Takes the pending "scroll to the current match" request.
    pub fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal)
    }

    /// Where the current match is on screen (its top-left) and the height of
    /// its line, from the last paint.
    pub fn current_hit_position(&self) -> Option<(Point<Pixels>, Pixels)> {
        let mut seen = 0;
        for entry in &self.entries {
            if self.current < seen + entry.hits.len() {
                let range = &entry.hits[self.current - seen];
                let at = entry.layout.position_for_index(range.start)?;
                return Some((at, entry.layout.line_height()));
            }
            seen += entry.hits.len();
        }
        None
    }

    /// The selected part of text `ix`, for drawing it highlighted.
    pub fn range_in(&self, ix: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = ordered(self.anchor?, self.head?);
        range_for(start, end, ix, len)
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Mouse down: a new selection starts (empty) where the pointer is.
    pub fn start(&mut self, at: Point<Pixels>) {
        self.clear();
        if let Some(pos) = self.hit(at) {
            self.anchor = Some(pos);
            self.head = Some(pos);
            self.dragging = true;
        }
    }

    /// Mouse move while pressed. Returns whether the selection changed.
    pub fn drag(&mut self, at: Point<Pixels>) -> bool {
        let Some(pos) = self.hit(at) else {
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

    pub fn select_all(&mut self) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            return;
        };
        self.anchor = Some((0, 0));
        self.head = Some((last, self.entries[last].layout.len()));
    }

    /// The text under the pointer, or the nearest one when it is between texts.
    fn hit(&self, at: Point<Pixels>) -> Option<Pos> {
        let mut best: Option<(usize, Pixels, Pixels)> = None;
        for (ix, entry) in self.entries.iter().enumerate() {
            let b = entry.layout.bounds();
            let dy = (b.top() - at.y).max(at.y - b.bottom()).max(Pixels::ZERO);
            let dx = (b.left() - at.x).max(at.x - b.right()).max(Pixels::ZERO);
            let closer = best.is_none_or(|(_, by, bx)| dy < by || (dy == by && dx < bx));
            if closer {
                best = Some((ix, dy, dx));
            }
        }
        let (ix, ..) = best?;
        let entry = &self.entries[ix];
        let b = entry.layout.bounds();
        // Clamp into the text so a pointer beside or between lines still picks
        // the nearest character.
        let inside = gpui::point(
            at.x.max(b.left()).min(b.right() - gpui::px(1.0)),
            at.y.max(b.top()).min(b.bottom() - gpui::px(1.0)),
        );
        let offset = entry
            .layout
            .index_for_position(inside)
            .unwrap_or_else(|nearest| nearest);
        Some((ix, if entry.blank { 0 } else { offset }))
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = ordered(self.anchor?, self.head?);
        if start == end {
            return None;
        }
        let last = end.0.min(self.entries.len().checked_sub(1)?);
        let parts: Vec<(String, &'static str)> = self.entries[start.0..=last]
            .iter()
            .map(|e| {
                let text = if e.blank {
                    String::new()
                } else {
                    e.layout.text()
                };
                (text, e.join)
            })
            .collect();
        Some(assemble(&parts, start.0, start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_through_matches_wraps_around() {
        assert_eq!(wrap_step(0, 3, 1), 1);
        assert_eq!(wrap_step(2, 3, 1), 0);
        assert_eq!(wrap_step(0, 3, -1), 2);
        assert_eq!(wrap_step(0, 0, 1), 0);
    }

    #[test]
    fn ends_are_put_in_document_order() {
        assert_eq!(ordered((3, 1), (1, 9)), ((1, 9), (3, 1)));
        assert_eq!(ordered((2, 4), (2, 1)), ((2, 1), (2, 4)));
    }

    #[test]
    fn a_selection_covers_whole_middle_texts_and_part_of_the_ends() {
        let (s, e) = ((1, 3), (3, 2));
        assert_eq!(range_for(s, e, 0, 10), None);
        assert_eq!(range_for(s, e, 1, 10), Some(3..10));
        assert_eq!(range_for(s, e, 2, 7), Some(0..7));
        assert_eq!(range_for(s, e, 3, 10), Some(0..2));
        assert_eq!(range_for(s, e, 4, 10), None);
        // Inside one text.
        assert_eq!(range_for((2, 1), (2, 4), 2, 10), Some(1..4));
        // An empty selection or an empty text covers nothing.
        assert_eq!(range_for((2, 4), (2, 4), 2, 10), None);
        assert_eq!(range_for(s, e, 2, 0), None);
    }

    #[test]
    fn copying_joins_the_covered_slices() {
        let parts = vec![
            ("Heading".to_string(), "\n\n"),
            ("first paragraph".to_string(), "\n\n"),
            ("".to_string(), "\n"),
            ("last line".to_string(), "\n"),
        ];
        // From "ing" in text 0 to "las" in text 3.
        let text = assemble(&parts, 0, (0, 4), (3, 3));
        assert_eq!(text, "ing\n\nfirst paragraph\n\nlas");
        // Starting partway through the list: only its own slice, no leading join.
        let text = assemble(&parts[1..2], 1, (1, 6), (1, 15));
        assert_eq!(text, "paragraph");
    }

    #[test]
    fn multibyte_slices_never_split_a_character() {
        let parts = vec![("héllo".to_string(), "\n")];
        // Byte 2 is inside "é": the slice is dropped rather than panicking.
        assert_eq!(assemble(&parts, 0, (0, 2), (0, 5)), "");
        assert_eq!(assemble(&parts, 0, (0, 1), (0, 3)), "é");
    }
}
