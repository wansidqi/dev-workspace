//! Markdown → a small block model for the note preview.
//!
//! Parsing is pure (no GPUI) so it can be unit tested; `markdown_view.rs`
//! turns the blocks into elements. Raw HTML in a note is shown as plain text,
//! never interpreted, since notes are untrusted input.

use std::cell::Cell;
use std::ops::Range;

use devspace_core::TaskPriority;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A run of text sharing one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    Code {
        lang: Option<String>,
        text: String,
    },
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Rule,
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `Some` for a task-list item (`- [ ]` / `- [x]`).
    pub checked: Option<bool>,
    /// Where the `[ ]` / `[x]` marker sits in the note text, so the preview can
    /// tick it. `None` when it cannot be located reliably.
    pub marker: Option<Range<usize>>,
    /// Priority written after the marker (`[ ] !high text`), already removed from
    /// the item text, and where that token sits in the note.
    pub priority: Option<TaskPriority>,
    pub priority_token: Option<Range<usize>>,
    pub blocks: Vec<Block>,
}

/// Longest note the preview will parse; beyond this the tail is cut so a huge
/// file cannot stall the UI thread.
const MAX_PREVIEW_BYTES: usize = 512 * 1024;
/// Nesting limit for quotes and lists, against pathological input.
const MAX_DEPTH: usize = 16;

/// Parses markdown into blocks.
pub fn parse(source: &str) -> Vec<Block> {
    let source = if source.len() > MAX_PREVIEW_BYTES {
        let mut end = MAX_PREVIEW_BYTES;
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        &source[..end]
    } else {
        source
    };
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let pos = Cell::new((0, 0));
    let mut events = Tracked {
        inner: Parser::new_ext(source, options).into_offset_iter(),
        pos: &pos,
    };
    let mut blocks = parse_blocks(&mut events, None, 0, &pos, true);
    annotate_priorities(&mut blocks, source);
    blocks
}

/// Passes events through while remembering the source range of the last one, so
/// task markers can be located without threading ranges through every parser.
struct Tracked<'p, I> {
    inner: I,
    pos: &'p Cell<(usize, usize)>,
}

impl<'a, 'p, I: Iterator<Item = (Event<'a>, Range<usize>)>> Iterator for Tracked<'p, I> {
    type Item = Event<'a>;
    fn next(&mut self) -> Option<Event<'a>> {
        let (event, range) = self.inner.next()?;
        self.pos.set((range.start, range.end));
        Some(event)
    }
}

#[derive(Default)]
struct Inline {
    spans: Vec<Span>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
}

impl Inline {
    fn push(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let span = Span {
            text: text.to_string(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
            link: self.link > 0,
        };
        // Merge neighbours with identical styling so the view builds fewer runs.
        if let Some(last) = self.spans.last_mut() {
            if last.bold == span.bold
                && last.italic == span.italic
                && last.strike == span.strike
                && last.code == span.code
                && last.link == span.link
            {
                last.text.push_str(&span.text);
                return;
            }
        }
        self.spans.push(span);
    }

    /// Consumes an inline event; returns false when the event is not inline.
    fn handle(&mut self, event: &Event) -> bool {
        match event {
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => self.push(t, false),
            Event::Code(t) => self.push(t, true),
            Event::SoftBreak => self.push(" ", false),
            Event::HardBreak => self.push("\n", false),
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => self.link += 1,
            Event::End(TagEnd::Link) => self.link = self.link.saturating_sub(1),
            // Images are shown as their alt text.
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            _ => return false,
        }
        true
    }

    fn take(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.spans)
    }
}

fn level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn inline_until<'a>(events: &mut impl Iterator<Item = Event<'a>>, end: TagEnd) -> Vec<Span> {
    let mut inline = Inline::default();
    for event in events.by_ref() {
        if event == Event::End(end) {
            break;
        }
        inline.handle(&event);
    }
    inline.take()
}

/// Splits a leading checkbox marker (`[ ]`, `[]`, `[x]`, `[X]`, `[✅]`) off
/// `text`, returning whether it is ticked and the text after it. Notes are
/// often written as `[ ] todo` lines with no `- ` in front, which markdown
/// itself would leave as plain text.
fn checkbox_prefix(text: &str) -> Option<(bool, &str)> {
    let rest = text.strip_prefix('[')?;
    let (done, rest) = if let Some(r) = rest.strip_prefix(']') {
        (false, r)
    } else if let Some(r) = rest.strip_prefix(" ]") {
        (false, r)
    } else {
        let end = rest.find(']')?;
        match &rest[..end] {
            "x" | "X" | "✅" => (true, &rest[end + 1..]),
            _ => return None,
        }
    };
    // Require a space after the marker so `[]()` or `[x]y` stay plain text.
    let rest = rest.strip_prefix(' ')?;
    Some((done, rest))
}

/// A paragraph, or a one-item task list when it starts with a checkbox marker.
fn paragraph_block(mut spans: Vec<Span>, start: Option<usize>) -> Block {
    let Some((done, rest, marker_len)) = spans
        .first()
        .and_then(|s| checkbox_prefix(&s.text).map(|(d, r)| (d, r, s.text.len() - r.len() - 1)))
    else {
        return Block::Paragraph(spans);
    };
    let rest = rest.to_string();
    if rest.is_empty() {
        spans.remove(0);
    } else {
        spans[0].text = rest;
    }
    Block::List {
        start: None,
        items: vec![Item {
            checked: Some(done),
            marker: start.map(|s| s..s + marker_len),
            priority: None,
            priority_token: None,
            blocks: vec![Block::Paragraph(spans)],
        }],
    }
}

fn parse_blocks<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    until: Option<TagEnd>,
    depth: usize,
    pos: &Cell<(usize, usize)>,
    live: bool,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    // Tight list items carry inline events with no paragraph around them.
    let mut pending = Inline::default();

    macro_rules! flush {
        () => {
            let spans = pending.take();
            if !spans.is_empty() {
                blocks.push(paragraph_block(spans, None));
            }
        };
    }

    while let Some(event) = events.next() {
        if pending.handle(&event) {
            continue;
        }
        match event {
            Event::End(end) if Some(end) == until => break,
            Event::Start(Tag::Heading { level: l, .. }) => {
                flush!();
                blocks.push(Block::Heading(
                    level(l),
                    inline_until(events, TagEnd::Heading(l)),
                ));
            }
            Event::Start(Tag::Paragraph) => {
                flush!();
                // Only valid while events stream straight from the parser (list items
                // are buffered first, which loses their ranges).
                let start = live.then(|| pos.get().0);
                let spans = inline_until(events, TagEnd::Paragraph);
                if !spans.is_empty() {
                    blocks.push(paragraph_block(spans, start));
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush!();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) if !l.trim().is_empty() => Some(l.trim().to_string()),
                    _ => None,
                };
                let mut text = String::new();
                for e in events.by_ref() {
                    match e {
                        Event::End(TagEnd::CodeBlock) => break,
                        Event::Text(t) => text.push_str(&t),
                        _ => {}
                    }
                }
                if text.ends_with('\n') {
                    text.pop();
                }
                blocks.push(Block::Code { lang, text });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush!();
                if depth >= MAX_DEPTH {
                    skip_until(events, TagEnd::BlockQuote(None));
                } else {
                    blocks.push(Block::Quote(parse_blocks(
                        events,
                        Some(TagEnd::BlockQuote(None)),
                        depth + 1,
                        pos,
                        live,
                    )));
                }
            }
            Event::Start(Tag::List(start)) => {
                flush!();
                if depth >= MAX_DEPTH {
                    skip_until(events, TagEnd::List(start.is_some()));
                    continue;
                }
                let mut items = Vec::new();
                while let Some(e) = events.next() {
                    match e {
                        Event::Start(Tag::Item) => {
                            let mut item_events = ItemEvents {
                                inner: events,
                                done: false,
                                open: 0,
                            };
                            let (item_blocks, item_checked, marker) =
                                parse_item(&mut item_events, depth + 1, pos);
                            items.push(Item {
                                checked: item_checked,
                                marker,
                                priority: None,
                                priority_token: None,
                                blocks: item_blocks,
                            });
                        }
                        Event::End(TagEnd::List(_)) => break,
                        _ => {}
                    }
                }
                blocks.push(Block::List { start, items });
            }
            Event::Rule => {
                flush!();
                blocks.push(Block::Rule);
            }
            Event::Start(Tag::Table(_)) => {
                flush!();
                blocks.push(parse_table(events));
            }
            // Anything else (footnotes, metadata, …) is ignored.
            _ => {}
        }
    }
    flush!();
    blocks
}

/// Wraps the event stream so an item's blocks stop at its closing tag.
struct ItemEvents<'b, I> {
    inner: &'b mut I,
    done: bool,
    /// Items opened inside this one (nested lists) that are still open.
    open: usize,
}

impl<'a, 'b, I: Iterator<Item = Event<'a>>> Iterator for ItemEvents<'b, I> {
    type Item = Event<'a>;
    fn next(&mut self) -> Option<Event<'a>> {
        if self.done {
            return None;
        }
        let event = self.inner.next()?;
        if event == Event::Start(Tag::Item) {
            self.open += 1;
        } else if event == Event::End(TagEnd::Item) {
            if self.open > 0 {
                self.open -= 1;
                return Some(event);
            }
            self.done = true;
            return None;
        }
        Some(event)
    }
}

fn parse_item<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    depth: usize,
    pos: &Cell<(usize, usize)>,
) -> (Vec<Block>, Option<bool>, Option<Range<usize>>) {
    // The task marker is the first event of the item; peel it off, then parse
    // the rest as ordinary blocks.
    let mut checked = None;
    let mut marker = None;
    let mut buffered: Vec<Event<'a>> = Vec::new();
    for event in events.by_ref() {
        if let Event::TaskListMarker(done) = event {
            checked = Some(done);
            let (start, end) = pos.get();
            marker = Some(start..end);
            continue;
        }
        buffered.push(event);
    }
    let mut iter = buffered.into_iter();
    (parse_blocks(&mut iter, None, depth, pos, false), checked, marker)
}

/// The word written after `!` for a priority (`!high`).
pub fn priority_name(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "med",
        TaskPriority::High => "high",
        TaskPriority::Urgent => "urgent",
    }
}

/// What a click on a task's priority chip moves to; none comes after urgent.
pub fn next_priority(current: Option<TaskPriority>) -> Option<TaskPriority> {
    match current {
        None => Some(TaskPriority::Low),
        Some(TaskPriority::Low) => Some(TaskPriority::Medium),
        Some(TaskPriority::Medium) => Some(TaskPriority::High),
        Some(TaskPriority::High) => Some(TaskPriority::Urgent),
        Some(TaskPriority::Urgent) => None,
    }
}

/// The priority token (`!low`, `!med`, `!medium`, `!high`, `!urgent`) written
/// right after a task marker ending at `marker_end`, with its source range.
fn priority_after(source: &str, marker_end: usize) -> Option<(TaskPriority, Range<usize>)> {
    let rest = source.get(marker_end..)?.strip_prefix(' ')?;
    let token = rest.split(char::is_whitespace).next()?;
    let priority = match token {
        "!low" => TaskPriority::Low,
        "!med" | "!medium" => TaskPriority::Medium,
        "!high" => TaskPriority::High,
        "!urgent" => TaskPriority::Urgent,
        _ => return None,
    };
    let start = marker_end + 1;
    Some((priority, start..start + token.len()))
}

/// Fills in each task's priority from the source and removes the token from the
/// text shown, so it renders as a chip instead of literal `!high`.
fn annotate_priorities(blocks: &mut [Block], source: &str) {
    for block in blocks {
        match block {
            Block::List { items, .. } => {
                for item in items {
                    if let (Some(_), Some(marker)) = (item.checked, item.marker.clone()) {
                        if let Some((priority, token)) = priority_after(source, marker.end) {
                            item.priority = Some(priority);
                            item.priority_token = Some(token.clone());
                            strip_leading_token(&mut item.blocks, source.get(token));
                        }
                    }
                    annotate_priorities(&mut item.blocks, source);
                }
            }
            Block::Quote(inner) => annotate_priorities(inner, source),
            _ => {}
        }
    }
}

fn strip_leading_token(blocks: &mut [Block], token: Option<&str>) {
    let (Some(token), Some(Block::Paragraph(spans))) = (token, blocks.first_mut()) else {
        return;
    };
    let Some(first) = spans.first_mut() else {
        return;
    };
    if let Some(rest) = first.text.strip_prefix(token) {
        first.text = rest.trim_start().to_string();
        if first.text.is_empty() {
            spans.remove(0);
        }
    }
}

/// (finished, total) task items anywhere in the note.
pub fn task_counts(blocks: &[Block]) -> (usize, usize) {
    let mut counts = (0, 0);
    for block in blocks {
        match block {
            Block::List { items, .. } => {
                for item in items {
                    if let Some(done) = item.checked {
                        counts.1 += 1;
                        counts.0 += usize::from(done);
                    }
                    let inner = task_counts(&item.blocks);
                    counts = (counts.0 + inner.0, counts.1 + inner.1);
                }
            }
            Block::Quote(inner) => {
                let inner = task_counts(inner);
                counts = (counts.0 + inner.0, counts.1 + inner.1);
            }
            _ => {}
        }
    }
    counts
}

/// (finished, total) tasks in raw note text, for the notes lists.
pub fn note_progress(source: &str) -> Option<(usize, usize)> {
    let counts = task_counts(&parse(source));
    (counts.1 > 0).then_some(counts)
}

/// A change the rendered preview asks the note text to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEdit {
    /// Tick or untick the task whose marker is at this range.
    Toggle(Range<usize>),
    /// Move a task to the next priority (see [`next_priority`]).
    CyclePriority {
        marker: Range<usize>,
        token: Option<Range<usize>>,
        current: Option<TaskPriority>,
    },
    /// Append a new unchecked task at the end of the note.
    Add(String),
}

/// The replacement a [`TaskEdit`] makes to `source`: byte range and new text.
/// `None` when the edit no longer applies (stale range, empty text), so a
/// preview that lags the text can never corrupt the note.
pub fn task_edit(source: &str, edit: &TaskEdit) -> Option<(Range<usize>, String)> {
    match edit {
        TaskEdit::Toggle(range) => {
            toggle_marker(source, range).map(|(range, text)| (range, text.to_string()))
        }
        TaskEdit::CyclePriority {
            marker,
            token,
            current,
        } => {
            let (marker, _) = toggle_marker(source, marker)?;
            if let (Some(token), Some(current)) = (token, current) {
                if source.get(token.clone())? != format!("!{}", priority_name(*current)) {
                    return None;
                }
            }
            match (token, next_priority(*current)) {
                (Some(token), Some(next)) => {
                    Some((token.clone(), format!("!{}", priority_name(next))))
                }
                (Some(token), None) => {
                    // Drop the token and the space after it.
                    let space = usize::from(source[token.end..].starts_with(' '));
                    Some((token.start..token.end + space, String::new()))
                }
                (None, Some(next)) => Some((
                    marker.end..marker.end,
                    format!(" !{}", priority_name(next)),
                )),
                (None, None) => None,
            }
        }
        TaskEdit::Add(text) => {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return None;
            }
            let body = source.trim_end();
            // Continue a `- [ ]` list in its own style; otherwise add a bare
            // task as its own paragraph (the style of the notes this serves).
            let list_style = body
                .lines()
                .next_back()
                .is_some_and(|l| l.trim_start().starts_with("- ["));
            let (sep, prefix) = match (body.is_empty(), list_style) {
                (true, _) => ("", "[ ] "),
                (false, true) => ("\n", "- [ ] "),
                (false, false) => ("\n\n", "[ ] "),
            };
            Some((body.len()..source.len(), format!("{sep}{prefix}{text}\n")))
        }
    }
}

/// The text edit that ticks or unticks the task marker inside `range` of
/// `source`: the byte range to replace and its replacement. `None` unless the
/// range really holds a marker, so a stale range can never corrupt the note.
pub fn toggle_marker(source: &str, range: &Range<usize>) -> Option<(Range<usize>, &'static str)> {
    const MARKERS: [(&str, bool); 5] = [
        ("[ ]", false),
        ("[]", false),
        ("[x]", true),
        ("[X]", true),
        ("[✅]", true),
    ];
    let slice = source.get(range.clone())?;
    let offset = slice.find('[')?;
    let tail = &slice[offset..];
    let (marker, done) = MARKERS.iter().find(|(m, _)| tail.starts_with(m))?;
    let start = range.start + offset;
    Some((start..start + marker.len(), if *done { "[ ]" } else { "[x]" }))
}

fn skip_until<'a>(events: &mut impl Iterator<Item = Event<'a>>, end: TagEnd) {
    let mut open = 1usize;
    for event in events.by_ref() {
        match &event {
            Event::Start(_) => open += 1,
            Event::End(e) => {
                open -= 1;
                if open == 0 && *e == end {
                    return;
                }
                if open == 0 {
                    return;
                }
            }
            _ => {}
        }
    }
}

fn parse_table<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Block {
    let mut header: Vec<Vec<Span>> = Vec::new();
    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut current: Vec<Vec<Span>> = Vec::new();
    let mut in_head = false;
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => {
                in_head = false;
                header = std::mem::take(&mut current);
            }
            Event::Start(Tag::TableRow) => current.clear(),
            Event::End(TagEnd::TableRow) if !in_head => rows.push(std::mem::take(&mut current)),
            Event::Start(Tag::TableCell) => current.push(inline_until(events, TagEnd::TableCell)),
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    Block::Table { header, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn headings_and_paragraphs() {
        let blocks = parse("## FIND long query\n\nhello **bold** and `code`");
        assert!(matches!(&blocks[0], Block::Heading(2, s) if text(s) == "FIND long query"));
        match &blocks[1] {
            Block::Paragraph(spans) => {
                assert_eq!(text(spans), "hello bold and code");
                assert!(spans.iter().any(|s| s.bold && s.text == "bold"));
                assert!(spans.iter().any(|s| s.code && s.text == "code"));
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn fenced_code_keeps_language_and_text() {
        let blocks = parse("```sql\nSELECT 1;\nSELECT 2;\n```\n");
        assert_eq!(
            blocks,
            vec![Block::Code {
                lang: Some("sql".into()),
                text: "SELECT 1;\nSELECT 2;".into()
            }]
        );
    }

    #[test]
    fn tight_and_task_lists() {
        let blocks = parse("- one\n- [x] done\n- [ ] todo\n");
        let Block::List { start, items } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].checked, None);
        assert_eq!(items[1].checked, Some(true));
        assert_eq!(items[2].checked, Some(false));
        assert!(matches!(&items[0].blocks[0], Block::Paragraph(s) if text(s) == "one"));
    }

    #[test]
    fn bare_checkbox_lines_become_tasks() {
        let blocks = parse("[ ] one\n\n[] two\n\n[x] three\n\n[✅] four\n\n[link](u) five\n\n[x]no\n");
        let checked: Vec<Option<bool>> = blocks
            .iter()
            .map(|b| match b {
                Block::List { items, .. } => items[0].checked,
                _ => None,
            })
            .collect();
        assert_eq!(
            checked,
            vec![Some(false), Some(false), Some(true), Some(true), None, None]
        );
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected task");
        };
        assert!(matches!(&items[0].blocks[0], Block::Paragraph(s) if text(s) == "one"));
    }

    /// Applies the toggle for the `index`th task found in `source`.
    fn toggled(source: &str, index: usize) -> String {
        let mut tasks = Vec::new();
        fn collect(blocks: &[Block], out: &mut Vec<Range<usize>>) {
            for b in blocks {
                match b {
                    Block::List { items, .. } => {
                        for item in items {
                            if let (Some(_), Some(m)) = (item.checked, &item.marker) {
                                out.push(m.clone());
                            }
                            collect(&item.blocks, out);
                        }
                    }
                    Block::Quote(inner) => collect(inner, out),
                    _ => {}
                }
            }
        }
        collect(&parse(source), &mut tasks);
        let (range, text) = toggle_marker(source, &tasks[index]).expect("marker located");
        let mut out = source.to_string();
        out.replace_range(range, text);
        out
    }

    #[test]
    fn toggling_a_task_rewrites_only_its_marker() {
        let src = "intro\n\n- [ ] a\n- [x] b\n\n[ ] bare\n\n[✅] done\n\n```\n[ ] not a task\n```\n";
        assert_eq!(toggled(src, 0), src.replacen("- [ ] a", "- [x] a", 1));
        assert_eq!(toggled(src, 1), src.replacen("- [x] b", "- [ ] b", 1));
        assert_eq!(toggled(src, 2), src.replacen("[ ] bare", "[x] bare", 1));
        assert_eq!(toggled(src, 3), src.replacen("[✅] done", "[ ] done", 1));
        // A range that does not hold a marker is refused.
        assert!(toggle_marker("hello", &(0..3)).is_none());
        assert!(toggle_marker("hello", &(2..99)).is_none());
    }

    fn first_task(source: &str) -> Item {
        let blocks = parse(source);
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected a task");
        };
        items[0].clone()
    }

    fn applied(source: &str, edit: &TaskEdit) -> String {
        let (range, text) = task_edit(source, edit).expect("edit applies");
        let mut out = source.to_string();
        out.replace_range(range, &text);
        out
    }

    #[test]
    fn priority_is_read_from_the_source_and_hidden_from_the_text() {
        for (src, want) in [
            ("[ ] !high fix login", TaskPriority::High),
            ("- [x] !urgent ship it", TaskPriority::Urgent),
            ("[] !med look", TaskPriority::Medium),
            ("[ ] !low later", TaskPriority::Low),
        ] {
            let item = first_task(src);
            assert_eq!(item.priority, Some(want), "{src}");
            let Block::Paragraph(spans) = &item.blocks[0] else {
                panic!("expected text");
            };
            assert!(!text(spans).contains('!'), "{src}: {}", text(spans));
        }
        let plain = first_task("[ ] !hot take");
        assert_eq!(plain.priority, None);
        let Block::Paragraph(spans) = &plain.blocks[0] else {
            panic!("expected text");
        };
        assert_eq!(text(spans), "!hot take");
    }

    #[test]
    fn cycling_priority_walks_none_to_urgent_and_back_to_none() {
        let mut src = "- [ ] write docs\n".to_string();
        let mut seen = Vec::new();
        for _ in 0..5 {
            let item = first_task(&src);
            seen.push(item.priority);
            src = applied(
                &src,
                &TaskEdit::CyclePriority {
                    marker: item.marker.clone().unwrap(),
                    token: item.priority_token.clone(),
                    current: item.priority,
                },
            );
        }
        assert_eq!(
            seen,
            vec![
                None,
                Some(TaskPriority::Low),
                Some(TaskPriority::Medium),
                Some(TaskPriority::High),
                Some(TaskPriority::Urgent),
            ]
        );
        assert_eq!(src, "- [ ] write docs\n", "back to no priority, text intact");
    }

    #[test]
    fn stale_priority_edits_are_refused() {
        let item = first_task("[ ] !high a");
        let edit = TaskEdit::CyclePriority {
            marker: item.marker.clone().unwrap(),
            token: item.priority_token.clone(),
            current: Some(TaskPriority::Low),
        };
        assert!(task_edit("[ ] !high a", &edit).is_none());
    }

    #[test]
    fn adding_a_task_appends_in_the_notes_own_style() {
        let add = |s: &str| applied(s, &TaskEdit::Add("  new   thing ".into()));
        assert_eq!(add(""), "[ ] new thing\n");
        assert_eq!(add("[ ] a\n"), "[ ] a\n\n[ ] new thing\n");
        assert_eq!(add("- [x] a\n- [ ] b\n\n"), "- [x] a\n- [ ] b\n- [ ] new thing\n");
        assert_eq!(add("intro"), "intro\n\n[ ] new thing\n");
        assert!(task_edit("x", &TaskEdit::Add("   ".into())).is_none());
    }

    #[test]
    fn counts_and_progress() {
        assert_eq!(note_progress("no tasks here"), None);
        assert_eq!(note_progress("[ ] a\n\n[x] b\n\n- [x] c\n"), Some((2, 3)));
    }

    #[test]
    fn ordered_list_start_and_nesting() {
        let blocks = parse("3. a\n   - nested\n4. b\n");
        let Block::List { start, items } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(*start, Some(3));
        assert_eq!(items.len(), 2);
        assert!(items[0]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::List { .. })));
    }

    #[test]
    fn quote_rule_and_table() {
        let blocks = parse("> quoted\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(matches!(&blocks[0], Block::Quote(inner) if !inner.is_empty()));
        assert_eq!(blocks[1], Block::Rule);
        let Block::Table { header, rows } = &blocks[2] else {
            panic!("expected table");
        };
        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0][1]), "2");
    }

    #[test]
    fn html_is_shown_as_text_not_interpreted() {
        let blocks = parse("<script>alert(1)</script>\n\nhi <b>x</b>");
        let joined: String = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(s) => Some(text(s)),
                _ => None,
            })
            .collect();
        assert!(joined.contains("<script>"));
    }

    #[test]
    fn deep_nesting_and_huge_input_do_not_overflow() {
        let deep = "> ".repeat(500) + "x";
        let _ = parse(&deep);
        let nested = (0..200)
            .map(|i| format!("{}- a\n", "  ".repeat(i)))
            .collect::<String>();
        let _ = parse(&nested);
        let huge = "word ".repeat(300_000);
        let _ = parse(&huge);
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(parse("").is_empty());
    }
}
