//! Markdown → a small block model for the note preview.
//!
//! Parsing is pure (no GPUI) so it can be unit tested; `markdown_view.rs`
//! turns the blocks into elements. Raw HTML in a note is shown as plain text,
//! never interpreted, since notes are untrusted input.

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
    let mut events = Parser::new_ext(source, options);
    parse_blocks(&mut events, None, 0)
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

fn parse_blocks<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    until: Option<TagEnd>,
    depth: usize,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    // Tight list items carry inline events with no paragraph around them.
    let mut pending = Inline::default();

    macro_rules! flush {
        () => {
            let spans = pending.take();
            if !spans.is_empty() {
                blocks.push(Block::Paragraph(spans));
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
                let spans = inline_until(events, TagEnd::Paragraph);
                if !spans.is_empty() {
                    blocks.push(Block::Paragraph(spans));
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
                            let (item_blocks, item_checked) =
                                parse_item(&mut item_events, depth + 1);
                            items.push(Item {
                                checked: item_checked,
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
) -> (Vec<Block>, Option<bool>) {
    // The task marker is the first event of the item; peel it off, then parse
    // the rest as ordinary blocks.
    let mut checked = None;
    let mut buffered: Vec<Event<'a>> = Vec::new();
    for event in events.by_ref() {
        if let Event::TaskListMarker(done) = event {
            checked = Some(done);
            continue;
        }
        buffered.push(event);
    }
    let mut iter = buffered.into_iter();
    (parse_blocks(&mut iter, None, depth), checked)
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
