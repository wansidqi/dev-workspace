//! Markdown → a small block model for the note preview.
//!
//! Parsing is pure (no GPUI) so it can be unit tested; `markdown_view.rs`
//! turns the blocks into elements. The target is GitHub's "Basic writing and
//! formatting syntax" page: headings, bold/italic/strikethrough, `<ins>`,
//! `<sub>`, `<sup>`, quotes, alerts (`> [!NOTE]`), inline and fenced code,
//! colour swatches for `#RRGGBB` / `rgb()` / `hsl()` in backticks, links and
//! bare URLs, line breaks, lists, `- [ ]` task items, tables, footnotes,
//! `:emoji:` shortcodes and `<!-- hidden comments -->`. Other raw HTML in a
//! note is shown as plain text, never interpreted, since notes are untrusted
//! input.

use std::cell::{Cell, RefCell};
use std::ops::Range;

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

/// A run of text sharing one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    /// `<ins>`: underlined text.
    pub underline: bool,
    /// For inline code that is a supported colour (`#0969DA`, `rgb(...)`,
    /// `hsl(...)`): the colour, drawn as a swatch before the text.
    pub swatch: Option<[u8; 3]>,
}

/// The five GitHub alert types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The title shown on the alert.
    pub fn title(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }
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
    Alert {
        kind: AlertKind,
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Rule,
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    /// A footnote definition as written; `parse` gathers these into
    /// [`Block::Footnotes`] at the end of the note.
    FootnoteDef {
        name: String,
        blocks: Vec<Block>,
    },
    /// The footnotes, numbered in order of first reference.
    Footnotes(Vec<(usize, Vec<Block>)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `Some` for a task-list item (`- [ ]` / `- [x]`), drawn as a checkbox.
    pub checked: Option<bool>,
    /// Where the `[ ]` / `[x]` marker sits in the note text, so the preview can
    /// tick it. `None` when it cannot be located.
    pub marker: Option<Range<usize>>,
    pub blocks: Vec<Block>,
}

/// Longest note the preview will parse; beyond this the tail is cut so a huge
/// file cannot stall the UI thread.
const MAX_PREVIEW_BYTES: usize = 512 * 1024;
/// Nesting limit for quotes and lists, against pathological input.
const MAX_DEPTH: usize = 16;

thread_local! {
    /// Footnote names in order of first reference, for the current `parse`.
    static FOOTNOTE_ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The 1-based number of a footnote, assigned on first use.
fn footnote_number(name: &str) -> usize {
    FOOTNOTE_ORDER.with(|order| {
        let mut order = order.borrow_mut();
        match order.iter().position(|n| n == name) {
            Some(i) => i + 1,
            None => {
                order.push(name.to_string());
                order.len()
            }
        }
    })
}

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
    FOOTNOTE_ORDER.with(|order| order.borrow_mut().clear());
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    let pos = Cell::new((0, 0));
    let mut events = Tracked {
        inner: Parser::new_ext(source, options).into_offset_iter(),
        pos: &pos,
    };
    let blocks = parse_blocks(&mut events, None, 0, &pos);

    // Footnote definitions move to the end, numbered by first reference.
    let mut body = Vec::with_capacity(blocks.len());
    let mut notes: Vec<(usize, Vec<Block>)> = Vec::new();
    for block in blocks {
        match block {
            Block::FootnoteDef { name, blocks } => notes.push((footnote_number(&name), blocks)),
            other => body.push(other),
        }
    }
    if !notes.is_empty() {
        notes.sort_by_key(|(n, _)| *n);
        body.push(Block::Footnotes(notes));
    }
    body
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

/// Replays buffered events with the range each had, keeping `pos` current.
struct Replay<'a, 'p> {
    events: std::vec::IntoIter<(Event<'a>, (usize, usize))>,
    pos: &'p Cell<(usize, usize)>,
}

impl<'a, 'p> Iterator for Replay<'a, 'p> {
    type Item = Event<'a>;
    fn next(&mut self) -> Option<Event<'a>> {
        let (event, range) = self.events.next()?;
        self.pos.set(range);
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
    ins: u32,
    sub: u32,
    sup: u32,
    /// Inside a `<!-- ... -->` that spans several events.
    in_comment: bool,
}

impl Inline {
    fn push_styled(&mut self, text: &str, code: bool, link: bool, swatch: Option<[u8; 3]>) {
        if text.is_empty() {
            return;
        }
        let text = if !code && (self.sub > 0 || self.sup > 0) {
            text.chars().map(|c| script_char(c, self.sup > 0)).collect()
        } else {
            text.to_string()
        };
        let span = Span {
            text,
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
            link: link || self.link > 0,
            underline: self.ins > 0,
            swatch,
        };
        // Merge neighbours with identical styling so the view builds fewer runs.
        if let Some(last) = self.spans.last_mut() {
            if last.swatch.is_none()
                && span.swatch.is_none()
                && last.bold == span.bold
                && last.italic == span.italic
                && last.strike == span.strike
                && last.code == span.code
                && last.link == span.link
                && last.underline == span.underline
            {
                last.text.push_str(&span.text);
                return;
            }
        }
        self.spans.push(span);
    }

    /// Plain text: `:emoji:` shortcodes become emoji and bare URLs become links.
    fn push_text(&mut self, text: &str) {
        let text = emojify(text);
        if self.link > 0 {
            self.push_styled(&text, false, false, None);
            return;
        }
        let mut rest = text.as_str();
        while let Some(at) = find_url(rest) {
            let (before, tail) = rest.split_at(at);
            let len = url_len(tail);
            self.push_styled(before, false, false, None);
            self.push_styled(&tail[..len], false, true, None);
            rest = &tail[len..];
        }
        self.push_styled(rest, false, false, None);
    }

    /// Inline HTML: the tags GitHub's page documents are honoured, comments are
    /// hidden, anything else is shown as text.
    fn html(&mut self, raw: &str) {
        if self.in_comment {
            if raw.contains("-->") {
                self.in_comment = false;
            }
            return;
        }
        let trimmed = raw.trim();
        if trimmed.starts_with("<!--") {
            self.in_comment = !trimmed.contains("-->");
            return;
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "<ins>" => self.ins += 1,
            "</ins>" => self.ins = self.ins.saturating_sub(1),
            "<sub>" => self.sub += 1,
            "</sub>" => self.sub = self.sub.saturating_sub(1),
            "<sup>" => self.sup += 1,
            "</sup>" => self.sup = self.sup.saturating_sub(1),
            "<br>" | "<br/>" | "<br />" => self.push_styled("\n", false, false, None),
            _ => self.push_styled(raw, false, false, None),
        }
    }

    /// Consumes an inline event; returns false when the event is not inline.
    fn handle(&mut self, event: &Event) -> bool {
        match event {
            Event::Text(t) => {
                if !self.in_comment {
                    self.push_text(t);
                }
            }
            Event::Html(t) | Event::InlineHtml(t) => self.html(t),
            Event::Code(t) => self.push_styled(t, true, false, color_of(t)),
            Event::SoftBreak => self.push_styled(" ", false, false, None),
            Event::HardBreak => self.push_styled("\n", false, false, None),
            Event::FootnoteReference(name) => {
                let number = footnote_number(name).to_string();
                self.sup += 1;
                self.push_styled(&number, false, true, None);
                self.sup -= 1;
            }
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

/// Superscript or subscript form of `c` where Unicode has one.
fn script_char(c: char, sup: bool) -> char {
    const SUP: &str = "0123456789+-=()in";
    const SUP_TO: &str = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁱⁿ";
    const SUB: &str = "0123456789+-=()aehiklmnoprstux";
    const SUB_TO: &str = "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢₖₗₘₙₒₚᵣₛₜᵤᵥₓ";
    let (from, to) = if sup { (SUP, SUP_TO) } else { (SUB, SUB_TO) };
    match from.chars().position(|f| f == c) {
        Some(i) => to.chars().nth(i).unwrap_or(c),
        None => c,
    }
}

/// Replaces `:name:` with its emoji for the common GitHub shortcodes; unknown
/// names are left as written.
fn emojify(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(':') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-')))
            .unwrap_or(after.len());
        if name_len > 0 && after[name_len..].starts_with(':') {
            if let Some(emoji) = emoji_for(&after[..name_len]) {
                out.push_str(emoji);
                rest = &after[name_len + 1..];
                continue;
            }
        }
        out.push(':');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn emoji_for(name: &str) -> Option<&'static str> {
    Some(match name {
        "+1" | "thumbsup" => "👍",
        "-1" | "thumbsdown" => "👎",
        "tada" => "🎉",
        "shipit" => "🐿️",
        "white_check_mark" => "✅",
        "heavy_check_mark" => "✔️",
        "x" => "❌",
        "warning" => "⚠️",
        "rocket" => "🚀",
        "fire" => "🔥",
        "bug" => "🐛",
        "memo" => "📝",
        "sparkles" => "✨",
        "eyes" => "👀",
        "heart" => "❤️",
        "smile" => "😄",
        "bulb" => "💡",
        "lock" => "🔒",
        "key" => "🔑",
        "star" => "⭐",
        "construction" => "🚧",
        "hammer" => "🔨",
        "wrench" => "🔧",
        "book" => "📖",
        "pushpin" => "📌",
        "question" => "❓",
        "exclamation" => "❗",
        "zap" => "⚡",
        "boom" => "💥",
        "clap" => "👏",
        "pray" => "🙏",
        "wave" => "👋",
        "100" => "💯",
        "thinking" => "🤔",
        "point_right" => "👉",
        "arrow_right" => "➡️",
        "link" => "🔗",
        _ => return None,
    })
}

/// Byte offset of the next `http://` or `https://` that starts a word.
fn find_url(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find("http") {
        let at = from + i;
        let tail = &text[at..];
        let starts_word = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '<' | '"' | '\''));
        let has_scheme = tail.starts_with("http://") || tail.starts_with("https://");
        let scheme_len = if tail.starts_with("https://") { 8 } else { 7 };
        if starts_word && has_scheme && tail.len() > scheme_len {
            return Some(at);
        }
        from = at + 4;
    }
    None
}

/// Length of the URL at the start of `tail`: up to whitespace, without the
/// sentence punctuation that usually follows a pasted link.
fn url_len(tail: &str) -> usize {
    let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
    let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', '"', '\'']);
    url.len()
}

/// The colour of a supported colour model written as inline code, as RGB:
/// `#RRGGBB`, `rgb(R,G,B)` or `hsl(H,S%,L%)`. Like GitHub, spaces inside the
/// backticks (leading or trailing) turn it back into plain code.
fn color_of(code: &str) -> Option<[u8; 3]> {
    if code != code.trim() {
        return None;
    }
    if let Some(hex) = code.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
        return None;
    }
    let lower = code.to_ascii_lowercase();
    if let Some(args) = lower.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if let [r, g, b] = parts[..] {
            return Some([r.parse().ok()?, g.parse().ok()?, b.parse().ok()?]);
        }
        return None;
    }
    if let Some(args) = lower.strip_prefix("hsl(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if let [h, s, l] = parts[..] {
            let h: f32 = h.parse().ok()?;
            let s: f32 = s.strip_suffix('%')?.parse().ok()?;
            let l: f32 = l.strip_suffix('%')?.parse().ok()?;
            if !(0.0..=100.0).contains(&s) || !(0.0..=100.0).contains(&l) {
                return None;
            }
            return Some(hsl_to_rgb(h.rem_euclid(360.0), s / 100.0, l / 100.0));
        }
    }
    None
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
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

fn alert_kind(kind: BlockQuoteKind) -> AlertKind {
    match kind {
        BlockQuoteKind::Note => AlertKind::Note,
        BlockQuoteKind::Tip => AlertKind::Tip,
        BlockQuoteKind::Important => AlertKind::Important,
        BlockQuoteKind::Warning => AlertKind::Warning,
        BlockQuoteKind::Caution => AlertKind::Caution,
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
/// `text`: whether it is ticked, the text after it, and the marker's byte
/// length. Notes are often written as `[ ] todo` lines with no `- ` in front,
/// which markdown itself would leave as plain text. A space must follow, so
/// `[x](link)` and `[x]y` stay text.
fn checkbox_prefix(text: &str) -> Option<(bool, &str, usize)> {
    let rest = text.strip_prefix('[')?;
    let (done, after) = if let Some(r) = rest.strip_prefix(']') {
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
    let after = after.strip_prefix(' ')?;
    Some((done, after, text.len() - after.len() - 1))
}

/// A paragraph, or a one-item task list when it starts with a checkbox marker.
/// `start` is where the paragraph begins in the note, to locate the marker.
fn paragraph_block(mut spans: Vec<Span>, start: usize) -> Block {
    let Some((done, rest, marker_len)) = spans
        .first()
        .and_then(|s| checkbox_prefix(&s.text).map(|(d, r, n)| (d, r.to_string(), n)))
    else {
        return Block::Paragraph(spans);
    };
    if rest.is_empty() {
        spans.remove(0);
    } else {
        spans[0].text = rest;
    }
    Block::List {
        start: None,
        items: vec![Item {
            checked: Some(done),
            marker: Some(start..start + marker_len),
            blocks: vec![Block::Paragraph(spans)],
        }],
    }
}

fn parse_blocks<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    until: Option<TagEnd>,
    depth: usize,
    pos: &Cell<(usize, usize)>,
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
                // The start event's range begins where the paragraph does.
                let start = pos.get().0;
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
            Event::Start(Tag::BlockQuote(kind)) => {
                flush!();
                if depth >= MAX_DEPTH {
                    skip_until(events, TagEnd::BlockQuote(kind));
                } else {
                    let inner =
                        parse_blocks(events, Some(TagEnd::BlockQuote(kind)), depth + 1, pos);
                    blocks.push(match kind {
                        Some(k) => Block::Alert {
                            kind: alert_kind(k),
                            blocks: inner,
                        },
                        None => Block::Quote(inner),
                    });
                }
            }
            Event::Start(Tag::FootnoteDefinition(name)) => {
                flush!();
                let name = name.to_string();
                if depth >= MAX_DEPTH {
                    skip_until(events, TagEnd::FootnoteDefinition);
                } else {
                    let inner = parse_blocks(events, Some(TagEnd::FootnoteDefinition), depth + 1, pos);
                    blocks.push(Block::FootnoteDef {
                        name,
                        blocks: inner,
                    });
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
                            let (item_blocks, checked, marker) =
                                parse_item(&mut item_events, depth + 1, pos);
                            items.push(Item {
                                checked,
                                marker,
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
            // Anything else (metadata, …) is ignored.
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
    // The task marker is the item's first event; peel it off (only that one, so
    // a nested task cannot change this item's state), then parse the rest as
    // ordinary blocks. Each buffered event keeps its source range so nested
    // items can still locate their own markers.
    let mut checked = None;
    let mut marker = None;
    let mut buffered: Vec<(Event<'a>, (usize, usize))> = Vec::new();
    for event in events.by_ref() {
        if let Event::TaskListMarker(done) = event {
            if buffered.is_empty() {
                checked = Some(done);
                let (start, end) = pos.get();
                marker = Some(start..end);
                continue;
            }
        }
        buffered.push((event, pos.get()));
    }
    let mut replay = Replay {
        events: buffered.into_iter(),
        pos,
    };
    (parse_blocks(&mut replay, None, depth, pos), checked, marker)
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

    fn para(blocks: &[Block], i: usize) -> &[Span] {
        match &blocks[i] {
            Block::Paragraph(s) => s,
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn headings_and_paragraphs() {
        let blocks = parse("## FIND long query\n\nhello **bold** and `code`");
        assert!(matches!(&blocks[0], Block::Heading(2, s) if text(s) == "FIND long query"));
        let spans = para(&blocks, 1);
        assert_eq!(text(spans), "hello bold and code");
        assert!(spans.iter().any(|s| s.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| s.code && s.text == "code"));
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
    fn bare_checkbox_lines_become_clickable_tasks() {
        let src = "[ ] one\n\n[] two\n\n[x] three\n\n[✅] four\n\n[link](u) five\n\n[x]no\n";
        let blocks = parse(src);
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
        // Every spelling can be toggled in the note text.
        assert_eq!(toggled(src, 0), src.replacen("[ ] one", "[x] one", 1));
        assert_eq!(toggled(src, 1), src.replacen("[] two", "[x] two", 1));
        assert_eq!(toggled(src, 2), src.replacen("[x] three", "[ ] three", 1));
        assert_eq!(toggled(src, 3), src.replacen("[✅] four", "[ ] four", 1));
    }

    fn markers(source: &str) -> Vec<Range<usize>> {
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
                    Block::Quote(inner) | Block::Alert { blocks: inner, .. } => collect(inner, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        collect(&parse(source), &mut out);
        out
    }

    fn toggled(source: &str, index: usize) -> String {
        let (range, text) = toggle_marker(source, &markers(source)[index]).expect("marker");
        let mut out = source.to_string();
        out.replace_range(range, text);
        out
    }

    #[test]
    fn clicking_a_checkbox_rewrites_only_its_marker() {
        let src = "intro\n\n- [ ] a\n- [x] b\n  - [ ] nested\n- [X] c\n\n```\n- [ ] not a task\n```\n";
        assert_eq!(markers(src).len(), 4);
        assert_eq!(toggled(src, 0), src.replacen("- [ ] a", "- [x] a", 1));
        assert_eq!(toggled(src, 1), src.replacen("- [x] b", "- [ ] b", 1));
        assert_eq!(toggled(src, 2), src.replacen("- [ ] nested", "- [x] nested", 1));
        assert_eq!(toggled(src, 3), src.replacen("- [X] c", "- [ ] c", 1));
        assert!(toggle_marker("hello", &(0..3)).is_none());
        assert!(toggle_marker("hello", &(2..99)).is_none());
    }

    #[test]
    fn a_nested_task_does_not_change_its_parents_state() {
        let blocks = parse("- [ ] parent\n  - [x] child\n");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items[0].checked, Some(false));
        let Block::List { items: inner, .. } = &items[0].blocks[1] else {
            panic!("expected nested list");
        };
        assert_eq!(inner[0].checked, Some(true));
    }

    #[test]
    fn task_item_may_start_with_an_escaped_parenthesis() {
        let blocks = parse("- [ ] \\(Optional) Open a followup issue\n");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items[0].checked, Some(false));
        assert!(matches!(&items[0].blocks[0], Block::Paragraph(s) if text(s).starts_with("(Optional)")));
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
    fn colour_models_in_backticks_get_a_swatch() {
        let blocks = parse(
            "`#0969DA` `rgb(9, 105, 218)` `hsl(212, 92%, 45%)` `#fff` `rgb(1,2)` ` #0969DA` #0969DA\n",
        );
        let swatches: Vec<Option<[u8; 3]>> = para(&blocks, 0)
            .iter()
            .filter(|s| s.code)
            .map(|s| s.swatch)
            .collect();
        assert_eq!(swatches[0], Some([9, 105, 218]));
        assert_eq!(swatches[1], Some([9, 105, 218]));
        // hsl(212, 92%, 45%) is GitHub's blue: close to the hex above.
        let [r, g, b] = swatches[2].expect("hsl swatch");
        assert!(r < 20 && (95..115).contains(&g) && (215..230).contains(&b));
        // Shorthand hex, too few channels and padded spaces are not colours.
        assert_eq!(&swatches[3..], &[None, None, None]);
        // Without backticks it stays plain text.
        assert!(para(&blocks, 0).last().is_some_and(|s| !s.code && s.swatch.is_none()));
    }

    #[test]
    fn alerts_use_the_five_github_types() {
        for (word, kind) in [
            ("NOTE", AlertKind::Note),
            ("TIP", AlertKind::Tip),
            ("IMPORTANT", AlertKind::Important),
            ("WARNING", AlertKind::Warning),
            ("CAUTION", AlertKind::Caution),
        ] {
            let blocks = parse(&format!("> [!{word}]\n> Useful information.\n"));
            match &blocks[0] {
                Block::Alert { kind: k, blocks } => {
                    assert_eq!(*k, kind);
                    assert!(matches!(&blocks[0], Block::Paragraph(s) if text(s) == "Useful information."));
                }
                other => panic!("expected alert for {word}, got {other:?}"),
            }
        }
        assert!(matches!(parse("> [!BOGUS]\n> x\n")[0], Block::Quote(_)));
    }

    #[test]
    fn footnotes_are_numbered_by_reference_and_collected_at_the_end() {
        let blocks = parse("A[^b] and B[^a].\n\n[^a]: Second.\n\n[^b]: First.\n");
        assert_eq!(blocks.len(), 2);
        let spans = para(&blocks, 0);
        assert_eq!(text(spans), "A¹ and B².");
        let Block::Footnotes(notes) = &blocks[1] else {
            panic!("expected footnotes");
        };
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].0, 1);
        assert!(matches!(&notes[0].1[0], Block::Paragraph(s) if text(s) == "First."));
        assert_eq!(notes[1].0, 2);
    }

    #[test]
    fn html_comments_are_hidden() {
        let blocks = parse("a <!-- secret --> b\n\n<!-- multi\nline -->\n\nafter\n");
        assert_eq!(text(para(&blocks, 0)), "a  b");
        assert_eq!(blocks.len(), 2);
        assert_eq!(text(para(&blocks, 1)), "after");
    }

    #[test]
    fn documented_html_tags_style_text() {
        let blocks = parse("<ins>under</ins> x<sub>2</sub> x<sup>2</sup> a<br/>b");
        let spans = para(&blocks, 0);
        assert!(spans.iter().any(|s| s.underline && s.text == "under"));
        assert_eq!(text(spans), "under x₂ x² a\nb");
    }

    #[test]
    fn emoji_shortcodes_and_bare_urls() {
        let blocks = parse("Ship it :tada: :unknown: see https://example.com/a, ok");
        let spans = para(&blocks, 0);
        assert_eq!(text(spans), "Ship it 🎉 :unknown: see https://example.com/a, ok");
        let link = spans.iter().find(|s| s.link).expect("autolink");
        assert_eq!(link.text, "https://example.com/a");
    }

    #[test]
    fn escapes_and_line_breaks_follow_commonmark() {
        let blocks = parse("Let's rename \\*our-new-project\\* now\n\nfirst  \nsecond\n");
        assert_eq!(text(para(&blocks, 0)), "Let's rename *our-new-project* now");
        assert_eq!(text(para(&blocks, 1)), "first\nsecond");
    }

    #[test]
    fn other_html_is_shown_as_text_not_interpreted() {
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
