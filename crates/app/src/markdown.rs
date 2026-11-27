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
    /// Where a click on a link goes: the URL or `#anchor`. Bare URLs link to
    /// themselves.
    pub href: Option<String>,
    /// `<ins>`: underlined text.
    pub underline: bool,
    /// For inline code that is a supported colour (`#0969DA`, `rgb(...)`,
    /// `hsl(...)`): the colour, drawn as a swatch before the text.
    pub swatch: Option<[u8; 3]>,
}

/// The alert types: GitHub's five, and three of our own (`> [!TODO]`,
/// `> [!BUG]`, `> [!QUESTION]`) that GitHub shows as a plain quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
    Todo,
    Bug,
    Question,
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
            AlertKind::Todo => "To-do",
            AlertKind::Bug => "Bug",
            AlertKind::Question => "Question",
        }
    }

    /// The kind written as `word` in `[!WORD]`, in any case.
    pub fn from_word(word: &str) -> Option<AlertKind> {
        Some(match word.to_ascii_lowercase().as_str() {
            "note" => AlertKind::Note,
            "tip" => AlertKind::Tip,
            "important" => AlertKind::Important,
            "warning" => AlertKind::Warning,
            "caution" => AlertKind::Caution,
            "todo" => AlertKind::Todo,
            "bug" => AlertKind::Bug,
            "question" => AlertKind::Question,
            _ => return None,
        })
    }
}

/// Whether an alert can be folded, from the marker after its kind:
/// `> [!NOTE]-` starts folded, `> [!NOTE]+` starts open but can fold, and a
/// plain `> [!NOTE]` is always open. (The convention Obsidian uses.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    None,
    Open,
    Closed,
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
        fold: Fold,
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

/// Parses markdown into blocks (the app itself uses [`parse_mapped`]).
#[cfg(test)]
pub fn parse(source: &str) -> Vec<Block> {
    parse_mapped(source).0
}

/// Like [`parse`], and also says where each top-level block starts in
/// `source` (a byte offset; `None` for the synthesised footnotes block), so a
/// click in the preview can put the editor's cursor at the same place.
pub fn parse_mapped(source: &str) -> (Vec<Block>, Vec<Option<usize>>) {
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
    let (blocks, block_starts) = parse_blocks_at(&mut events, None, 0, &pos);

    // Footnote definitions move to the end, numbered by first reference.
    let mut body = Vec::with_capacity(blocks.len());
    let mut starts = Vec::with_capacity(blocks.len());
    let mut notes: Vec<(usize, Vec<Block>)> = Vec::new();
    for (block, start) in blocks.into_iter().zip(block_starts) {
        match block {
            Block::FootnoteDef { name, blocks } => notes.push((footnote_number(&name), blocks)),
            other => {
                body.push(other);
                starts.push(Some(start));
            }
        }
    }
    if !notes.is_empty() {
        notes.sort_by_key(|(n, _)| *n);
        body.push(Block::Footnotes(notes));
        starts.push(None);
    }
    (body, starts)
}

/// The editor offset for a double-click in the preview: on the clicked letter
/// when `clicked` (the rendered text under the pointer and the byte offset in
/// it) can be found in the block's source, else the block's start. `starts` is
/// [`parse_mapped`]'s second result; `None` when that block has no source (the
/// synthesised footnotes).
pub fn click_offset(
    source: &str,
    starts: &[Option<usize>],
    block: usize,
    clicked: Option<(String, usize)>,
) -> Option<usize> {
    let start = starts.get(block).copied().flatten()?;
    let Some((rendered, in_text)) = clicked else {
        return Some(start);
    };
    // The block runs to the next block's start (or the note's end).
    let end = starts
        .iter()
        .skip(block + 1)
        .find_map(|s| *s)
        .unwrap_or(source.len())
        .min(source.len());
    let slice = source.get(start..end)?;
    Some(start + locate_in_source(slice, &rendered, in_text))
}

/// Where in a block's markdown `source` the preview text `rendered` at byte
/// `offset` came from, so a double-click on a word lands the editor's cursor on
/// that word. Markup (`**`, links, bullets) is gone from the rendered text, so
/// this matches text, not positions: first the text just before the click
/// together with the clicked word, then the word alone (the occurrence at the
/// same relative place wins). Returns an offset into `source`; 0 (the block's
/// start) when nothing matches.
pub fn locate_in_source(source: &str, rendered: &str, offset: usize) -> usize {
    let mut offset = offset.min(rendered.len());
    while !rendered.is_char_boundary(offset) {
        offset -= 1;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let word_start = rendered[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let word_end = rendered[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(rendered.len(), |(i, _)| offset + i);
    let word = &rendered[word_start..word_end];
    if word.is_empty() {
        return 0;
    }
    let within = offset - word_start;

    // A few characters before the word tell repeated words apart.
    let mut ctx_start = word_start;
    for (i, _) in rendered[..word_start].char_indices().rev().take(12) {
        ctx_start = i;
    }
    let with_ctx = &rendered[ctx_start..word_end];
    if ctx_start < word_start {
        if let Some(at) = source.find(with_ctx) {
            return at + (word_start - ctx_start) + within;
        }
    }
    let ratio = word_start as f64 / rendered.len().max(1) as f64;
    source
        .match_indices(word)
        .map(|(at, _)| at)
        .min_by(|a, b| {
            let da = (*a as f64 / source.len().max(1) as f64 - ratio).abs();
            let db = (*b as f64 / source.len().max(1) as f64 - ratio).abs();
            da.total_cmp(&db)
        })
        .map_or(0, |at| at + within)
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
    /// Destinations of the links currently open, innermost last.
    hrefs: Vec<String>,
    ins: u32,
    sub: u32,
    sup: u32,
    /// Inside a `<!-- ... -->` that spans several events.
    in_comment: bool,
}

impl Inline {
    /// `autolink` is the URL for text that is itself a bare link; text inside a
    /// `[label](url)` takes the open link's URL.
    fn push_styled(
        &mut self,
        text: &str,
        code: bool,
        autolink: Option<&str>,
        swatch: Option<[u8; 3]>,
    ) {
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
            link: autolink.is_some() || self.link > 0,
            href: autolink
                .map(str::to_string)
                .or_else(|| self.hrefs.last().cloned()),
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
                && last.href == span.href
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
            self.push_styled(&text, false, None, None);
            return;
        }
        let mut rest = text.as_str();
        while let Some(at) = find_url(rest) {
            let (before, tail) = rest.split_at(at);
            let len = url_len(tail);
            self.push_styled(before, false, None, None);
            self.push_styled(&tail[..len], false, Some(&tail[..len]), None);
            rest = &tail[len..];
        }
        self.push_styled(rest, false, None, None);
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
            "<br>" | "<br/>" | "<br />" => self.push_styled("\n", false, None, None),
            _ => self.push_styled(raw, false, None, None),
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
            Event::Code(t) => self.push_styled(t, true, None, color_of(t)),
            Event::SoftBreak => self.push_styled(" ", false, None, None),
            Event::HardBreak => self.push_styled("\n", false, None, None),
            Event::FootnoteReference(name) => {
                let number = footnote_number(name).to_string();
                self.sup += 1;
                self.link += 1;
                self.push_styled(&number, false, None, None);
                self.link -= 1;
                self.sup -= 1;
            }
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.link += 1;
                self.hrefs.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                self.link = self.link.saturating_sub(1);
                self.hrefs.pop();
            }
            // Images are shown as their alt text.
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            _ => return false,
        }
        true
    }

    fn take(&mut self) -> Vec<Span> {
        split_task_numbers(split_refs(std::mem::take(&mut self.spans)))
    }
}

/// Turns a task number written as `#4` in plain text into a reference span (the
/// same kind a `[[Title]]` makes). The `#` must start the text or follow a
/// space or an opening bracket or comma, and the digits must be the whole word:
/// `#4a`, `#4_x`, `abc#4`, `#` alone and runs of more than
/// `refs::MAX_NUMBER_DIGITS` digits stay text. Code, links (a URL's `#4`
/// fragment included) and spans already turned into references are skipped.
/// Whether a number is really a task of the open project is decided when the
/// preview is drawn, so a stray `#12` in prose just reads as text.
fn split_task_numbers(spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if span.code || span.link || !span.text.contains('#') {
            out.push(span);
            continue;
        }
        let text = span.text.as_str();
        let piece = |part: &str| Span {
            text: part.to_string(),
            ..span.clone()
        };
        let mut from = 0;
        let mut search = 0;
        while let Some(found) = text[search..].find('#') {
            let hash = search + found;
            search = hash + 1;
            let before_ok = text[..hash]
                .chars()
                .next_back()
                .is_none_or(|c| c.is_whitespace() || "([{,;:\"'".contains(c));
            let digits = text[hash + 1..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            let end = hash + 1 + digits;
            let whole_word = text[end..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if !before_ok || digits == 0 || digits > crate::refs::MAX_NUMBER_DIGITS || !whole_word {
                continue;
            }
            if hash > from {
                out.push(piece(&text[from..hash]));
            }
            out.push(Span {
                text: text[hash..end].to_string(),
                link: true,
                href: Some(format!("{REF_SCHEME}{}", &text[hash..end])),
                ..span.clone()
            });
            from = end;
            search = end;
        }
        if from < text.len() {
            out.push(piece(&text[from..]));
        }
    }
    out
}

/// Prefix of the `href` that a `[[Title]]` reference carries. A reference
/// is a link to a note or task of the same project, resolved by title when it
/// is clicked, so it never reaches the OS (see `external::open_url`).
pub const REF_SCHEME: &str = "devspace-ref:";

/// The title a reference `href` points at, or `None` for any other link.
pub fn ref_target(href: &str) -> Option<&str> {
    href.strip_prefix(REF_SCHEME)
}

/// Turns `[[Title]]` and `[[Title|label]]` in plain text into link spans.
/// Adjacent plain text has already been merged into one span, so a reference
/// is never split across spans. Code, existing links and anything with a
/// newline or bracket inside stay as written.
fn split_refs(spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if span.code || span.link || !span.text.contains("[[") {
            out.push(span);
            continue;
        }
        let mut rest = span.text.as_str();
        let piece = |text: &str| Span {
            text: text.to_string(),
            ..span.clone()
        };
        while let Some(open) = rest.find("[[") {
            let inner = &rest[open + 2..];
            let Some(close) = inner.find("]]") else { break };
            let body = &inner[..close];
            let (target, label) = match body.split_once('|') {
                Some((t, l)) => (t.trim(), l.trim()),
                None => (body.trim(), body.trim()),
            };
            let valid = !target.is_empty() && !label.is_empty() && !body.contains(['\n', '[', ']']);
            if !valid {
                // Keep the brackets as text and look past this opener.
                let keep = open + 2;
                out.push(piece(&rest[..keep]));
                rest = &rest[keep..];
                continue;
            }
            if open > 0 {
                out.push(piece(&rest[..open]));
            }
            out.push(Span {
                text: label.to_string(),
                link: true,
                href: Some(format!("{REF_SCHEME}{target}")),
                ..span.clone()
            });
            rest = &inner[close + 2..];
        }
        if !rest.is_empty() {
            out.push(piece(rest));
        }
    }
    out
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

/// A quote whose first paragraph starts `[!KIND]`, `[!KIND]+` or `[!KIND]-`
/// becomes an alert of that kind, with the marker taken off the text; anything
/// else stays a quote. The marker must be a whole word, so `[!TODOS]` and
/// `[!NOTE]x` are text, and it must be plain text: inside code or a link it is
/// not a marker.
fn alert_from_marker(mut inner: Vec<Block>) -> Block {
    let marker = match inner.first() {
        Some(Block::Paragraph(spans)) => spans
            .first()
            .filter(|s| !s.code && !s.link)
            .and_then(|s| alert_marker(&s.text)),
        _ => None,
    };
    let Some((kind, fold, used)) = marker else {
        return Block::Quote(inner);
    };
    if let Some(Block::Paragraph(spans)) = inner.first_mut() {
        let rest = spans[0].text[used..].trim_start().to_string();
        if rest.is_empty() {
            spans.remove(0);
        } else {
            spans[0].text = rest;
        }
        if spans.is_empty() {
            inner.remove(0);
        }
    }
    Block::Alert {
        kind,
        fold,
        blocks: inner,
    }
}

/// The kind, fold marker and byte length of a `[!KIND]` marker at the start of
/// `text`, if there is one.
fn alert_marker(text: &str) -> Option<(AlertKind, Fold, usize)> {
    let rest = text.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = AlertKind::from_word(&rest[..close])?;
    let after = 2 + close + 1;
    let (fold, len) = match text[after..].chars().next() {
        Some('+') => (Fold::Open, after + 1),
        Some('-') => (Fold::Closed, after + 1),
        _ => (Fold::None, after),
    };
    let tail = &text[len..];
    (tail.is_empty() || tail.starts_with(char::is_whitespace)).then_some((kind, fold, len))
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
    parse_blocks_at(events, until, depth, pos).0
}

/// [`parse_blocks`] plus the source offset each returned block starts at (the
/// start of the event that opened it).
fn parse_blocks_at<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    until: Option<TagEnd>,
    depth: usize,
    pos: &Cell<(usize, usize)>,
) -> (Vec<Block>, Vec<usize>) {
    let mut blocks = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0;
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
        // Blocks added by the previous event began where that event did.
        while starts.len() < blocks.len() {
            starts.push(at);
        }
        at = pos.get().0;
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
                            fold: Fold::None,
                            blocks: inner,
                        },
                        // Our own kinds, and a fold marker, are not known to the
                        // parser, so they arrive as a quote that starts `[!KIND]`.
                        None => alert_from_marker(inner),
                    });
                }
            }
            Event::Start(Tag::FootnoteDefinition(name)) => {
                flush!();
                let name = name.to_string();
                if depth >= MAX_DEPTH {
                    skip_until(events, TagEnd::FootnoteDefinition);
                } else {
                    let inner =
                        parse_blocks(events, Some(TagEnd::FootnoteDefinition), depth + 1, pos);
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
    while starts.len() < blocks.len() {
        starts.push(at);
    }
    (blocks, starts)
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
    Some((
        start..start + marker.len(),
        if *done { "[ ]" } else { "[x]" },
    ))
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

/// GitHub's anchor for a heading: lowercase, spaces to hyphens, other
/// whitespace and punctuation removed (hyphens and underscores stay), markup
/// already gone because the heading is plain spans.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c == ' ' {
            out.push('-');
        } else if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The top-level block index of the heading a `#anchor` link points at. Repeated
/// headings get `-1`, `-2`, ... in order, as on GitHub. Only top-level headings
/// can be scrolled to.
pub fn anchor_index(blocks: &[Block], anchor: &str) -> Option<usize> {
    let want = anchor.trim_start_matches('#').to_lowercase();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, block) in blocks.iter().enumerate() {
        let Block::Heading(_, spans) = block else {
            continue;
        };
        let base = slug(&spans.iter().map(|s| s.text.as_str()).collect::<String>());
        let count = seen.entry(base.clone()).or_insert(0);
        let name = if *count == 0 {
            base
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        if name == want {
            return Some(i);
        }
    }
    None
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
    fn top_level_blocks_know_where_they_start() {
        let src = "# Title\n\npara one\n\n```\ncode\n```\n\n- a\n- b\n";
        let (blocks, starts) = parse_mapped(src);
        assert_eq!(blocks.len(), starts.len());
        let at = |s: &str| src.find(s);
        assert_eq!(starts, vec![Some(0), at("para"), at("```"), at("- a")]);
    }

    #[test]
    fn a_click_lands_on_the_clicked_word_and_letter() {
        let src = "I will refine the plan and refine it again";
        // Plain text maps one to one: the second "refine", on its "i".
        let rendered = src;
        let click = src.rfind("refine").unwrap() + 3;
        assert_eq!(locate_in_source(src, rendered, click), click);
        // The first "refine" is told apart from the second.
        let first = src.find("refine").unwrap() + 3;
        assert_eq!(locate_in_source(src, rendered, first), first);
    }

    #[test]
    fn markup_before_the_word_does_not_throw_the_match_off() {
        let src = "- **bold** and [a link](http://x.test) then refine here";
        let rendered = "bold and a link then refine here";
        let click = rendered.find("refine").unwrap() + 3;
        assert_eq!(
            locate_in_source(src, rendered, click),
            src.find("refine").unwrap() + 3
        );
        // Inside bold text.
        let click = rendered.find("bold").unwrap() + 1;
        assert_eq!(
            locate_in_source(src, rendered, click),
            src.find("bold").unwrap() + 1
        );
    }

    #[test]
    fn nothing_to_match_falls_back_to_the_block_start() {
        assert_eq!(locate_in_source("abc", "xyz", 1), 0);
        assert_eq!(locate_in_source("abc", "", 0), 0);
        // Multibyte text never splits a character.
        let src = "héllo wörld";
        let at = locate_in_source(src, src, 8);
        assert!(src.is_char_boundary(at));
    }

    #[test]
    fn footnote_definitions_do_not_shift_block_starts() {
        let src = "a[^1]\n\n[^1]: note\n\nb";
        let (blocks, starts) = parse_mapped(src);
        assert_eq!(blocks.len(), starts.len());
        assert_eq!(starts, vec![Some(0), src.rfind('b'), None]);
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
        let src =
            "intro\n\n- [ ] a\n- [x] b\n  - [ ] nested\n- [X] c\n\n```\n- [ ] not a task\n```\n";
        assert_eq!(markers(src).len(), 4);
        assert_eq!(toggled(src, 0), src.replacen("- [ ] a", "- [x] a", 1));
        assert_eq!(toggled(src, 1), src.replacen("- [x] b", "- [ ] b", 1));
        assert_eq!(
            toggled(src, 2),
            src.replacen("- [ ] nested", "- [x] nested", 1)
        );
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
    fn double_brackets_become_references() {
        let blocks = parse("See [[Deploy checklist]] and **[[Setup|the setup]]** now.");
        let spans = para(&blocks, 0);
        let refs: Vec<(&str, &str, bool)> = spans
            .iter()
            .filter_map(|s| ref_target(s.href.as_deref()?).map(|t| (s.text.as_str(), t, s.bold)))
            .collect();
        assert_eq!(
            refs,
            vec![
                ("Deploy checklist", "Deploy checklist", false),
                ("the setup", "Setup", true),
            ]
        );
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "See Deploy checklist and the setup now.");
    }

    #[test]
    fn task_numbers_become_references() {
        let blocks = parse("fix #4 and (#12), **#7**, [#5] too. Done: #8.");
        let spans = para(&blocks, 0);
        let refs: Vec<(&str, &str)> = spans
            .iter()
            .filter_map(|s| ref_target(s.href.as_deref()?).map(|t| (s.text.as_str(), t)))
            .collect();
        assert_eq!(
            refs,
            vec![
                ("#4", "#4"),
                ("#12", "#12"),
                ("#7", "#7"),
                ("#5", "#5"),
                ("#8", "#8")
            ]
        );
        // The surrounding text is untouched.
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "fix #4 and (#12), #7, [#5] too. Done: #8.");
        assert!(spans.iter().any(|s| s.bold && s.text == "#7"));
    }

    #[test]
    fn hash_text_that_is_not_a_task_number_stays_text() {
        for src in [
            "#4a",
            "x#4",
            "#4_x",
            "price #",
            "#1234567890",
            "see `#4`",
            "[#4](https://example.com)",
            "https://example.com/page#4",
            "C#4",
        ] {
            let blocks = parse(src);
            let any = match blocks.first() {
                Some(Block::Paragraph(spans)) => spans
                    .iter()
                    .any(|s| s.href.as_deref().and_then(ref_target).is_some()),
                _ => false,
            };
            assert!(!any, "{src:?} must not become a task reference");
        }
        // A heading marker is not a task number either.
        assert!(matches!(
            parse("# 4 things").first(),
            Some(Block::Heading(..))
        ));
    }

    #[test]
    fn broken_or_quoted_references_stay_text() {
        for src in [
            "[[]]",
            "[[ ]]",
            "[[open",
            "[[a  \nb]]",
            "`[[code]]`",
            "[[a[b]]",
        ] {
            let blocks = parse(src);
            let any = match blocks.first() {
                Some(Block::Paragraph(spans)) => spans
                    .iter()
                    .any(|s| s.href.as_deref().and_then(ref_target).is_some()),
                _ => false,
            };
            assert!(!any, "{src:?} must not become a reference");
        }
    }

    #[test]
    fn links_carry_their_destination() {
        let blocks = parse(
            "See [GitHub Pages](https://pages.github.com/) or [below](#sample-section) or https://example.com/x.",
        );
        let hrefs: Vec<(&str, &str)> = para(&blocks, 0)
            .iter()
            .filter_map(|s| s.href.as_deref().map(|h| (s.text.as_str(), h)))
            .collect();
        assert_eq!(
            hrefs,
            vec![
                ("GitHub Pages", "https://pages.github.com/"),
                ("below", "#sample-section"),
                ("https://example.com/x", "https://example.com/x"),
            ]
        );
        // Plain text around them has no destination.
        assert!(para(&blocks, 0)[0].href.is_none());
    }

    #[test]
    fn heading_anchors_follow_githubs_rules() {
        let blocks = parse(
            "# Intro\n\n## This'll be a _Helpful_ Section About the Greek Letter Θ!\n\n## Not unique\n\ntext\n\n## Not unique\n\n## Not unique\n",
        );
        assert_eq!(anchor_index(&blocks, "intro"), Some(0));
        assert_eq!(
            anchor_index(
                &blocks,
                "thisll-be-a-helpful-section-about-the-greek-letter-θ"
            ),
            Some(1)
        );
        assert_eq!(anchor_index(&blocks, "not-unique"), Some(2));
        assert_eq!(anchor_index(&blocks, "not-unique-1"), Some(4));
        assert_eq!(anchor_index(&blocks, "not-unique-2"), Some(5));
        assert_eq!(anchor_index(&blocks, "#INTRO"), Some(0));
        assert_eq!(anchor_index(&blocks, "missing"), None);
    }

    /// `samples/markdown-demo.md` shows every supported feature; this keeps it
    /// honest. Skipped when the samples folder is not there.
    #[test]
    fn the_demo_note_exercises_every_feature() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/markdown-demo.md");
        let Ok(source) = std::fs::read_to_string(path) else {
            return;
        };
        let blocks = parse(&source);

        // Every `](#anchor)` link in the note points at a real heading.
        let mut rest = source.as_str();
        let mut anchors = 0;
        while let Some(at) = rest.find("](#") {
            let tail = &rest[at + 3..];
            let end = tail.find(')').expect("closed link");
            let name = &tail[..end];
            assert!(anchor_index(&blocks, name).is_some(), "dead anchor #{name}");
            anchors += 1;
            rest = &tail[end..];
        }
        assert!(
            anchors >= 15,
            "expected the contents list and more, got {anchors}"
        );

        // GitHub's five, our three, and one folded and one foldable example.
        let alerts: Vec<(AlertKind, Fold)> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Alert { kind, fold, .. } => Some((*kind, *fold)),
                _ => None,
            })
            .collect();
        assert_eq!(alerts.len(), 10);
        for kind in [AlertKind::Todo, AlertKind::Bug, AlertKind::Question] {
            assert!(alerts.iter().any(|(k, _)| *k == kind), "{kind:?}");
        }
        assert!(alerts.iter().any(|(_, f)| *f == Fold::Closed));
        assert!(alerts.iter().any(|(_, f)| *f == Fold::Open));

        // The references it shows are found, and the ones it only describes in
        // code are not.
        fn targets(blocks: &[Block], out: &mut Vec<String>) {
            for b in blocks {
                match b {
                    Block::Paragraph(s) | Block::Heading(_, s) => out.extend(
                        s.iter()
                            .filter_map(|s| s.href.as_deref().and_then(ref_target))
                            .map(str::to_string),
                    ),
                    Block::List { items, .. } => {
                        for item in items {
                            targets(&item.blocks, out);
                        }
                    }
                    Block::Quote(inner) | Block::Alert { blocks: inner, .. } => targets(inner, out),
                    _ => {}
                }
            }
        }
        let mut refs = Vec::new();
        targets(&blocks, &mut refs);
        for wanted in [
            "Markdown preview demo",
            "Release › Deploy › Plan",
            "Deploy > Plan",
            "A note that does not exist",
            "#1",
            "#99999",
        ] {
            assert!(
                refs.iter().any(|r| r == wanted),
                "missing {wanted:?}: {refs:?}"
            );
        }
        assert!(
            !refs.iter().any(|r| r == "#4" || r == "Title"),
            "code and glued numbers are not references: {refs:?}"
        );
        assert!(matches!(blocks.last(), Some(Block::Footnotes(n)) if n.len() == 2));
        assert!(markers(&source).len() >= 12, "tasks of every spelling");
        assert!(blocks.iter().any(|b| matches!(b, Block::Table { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, Block::Code { lang: Some(l), .. } if l == "sql")));
        // The raw script tag is shown as text, never dropped or run.
        assert!(blocks.iter().any(|b| matches!(b, Block::Paragraph(s)
            if s.iter().any(|s| s.text.contains("<script>")))));
    }

    #[test]
    fn task_item_may_start_with_an_escaped_parenthesis() {
        let blocks = parse("- [ ] \\(Optional) Open a followup issue\n");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items[0].checked, Some(false));
        assert!(
            matches!(&items[0].blocks[0], Block::Paragraph(s) if text(s).starts_with("(Optional)"))
        );
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
        assert!(para(&blocks, 0)
            .last()
            .is_some_and(|s| !s.code && s.swatch.is_none()));
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
                Block::Alert {
                    kind: k,
                    fold,
                    blocks,
                } => {
                    assert_eq!(*k, kind);
                    assert_eq!(*fold, Fold::None);
                    assert!(
                        matches!(&blocks[0], Block::Paragraph(s) if text(s) == "Useful information.")
                    );
                }
                other => panic!("expected alert for {word}, got {other:?}"),
            }
        }
        assert!(matches!(parse("> [!BOGUS]\n> x\n")[0], Block::Quote(_)));
    }

    /// The alert at the top of `src`: its kind, fold marker and body text.
    fn alert(src: &str) -> Option<(AlertKind, Fold, Vec<String>)> {
        match parse(src).into_iter().next()? {
            Block::Alert { kind, fold, blocks } => {
                let body = blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::Paragraph(s) => Some(text(s)),
                        _ => None,
                    })
                    .collect();
                Some((kind, fold, body))
            }
            _ => None,
        }
    }

    #[test]
    fn our_own_alert_kinds_are_recognised() {
        for (word, kind) in [
            ("TODO", AlertKind::Todo),
            ("BUG", AlertKind::Bug),
            ("QUESTION", AlertKind::Question),
            ("todo", AlertKind::Todo),
            ("Bug", AlertKind::Bug),
        ] {
            let got = alert(&format!("> [!{word}]\n> Needs a look.\n"));
            assert_eq!(
                got,
                Some((kind, Fold::None, vec!["Needs a look.".to_string()])),
                "{word}"
            );
        }
        // Text on the marker's own line is the body too.
        assert_eq!(
            alert("> [!BUG] Crash on save"),
            Some((
                AlertKind::Bug,
                Fold::None,
                vec!["Crash on save".to_string()]
            ))
        );
        // Several paragraphs and inline markup survive.
        let got = alert("> [!QUESTION]\n> First **bold**.\n>\n> Second.\n").expect("alert");
        assert_eq!(got.2, ["First bold.", "Second."]);
    }

    #[test]
    fn fold_markers_choose_whether_an_alert_can_fold() {
        let fold = |src: &str| alert(src).map(|a| a.1);
        assert_eq!(fold("> [!NOTE]-\n> x"), Some(Fold::Closed));
        assert_eq!(fold("> [!NOTE]+\n> x"), Some(Fold::Open));
        assert_eq!(fold("> [!NOTE]\n> x"), Some(Fold::None));
        assert_eq!(fold("> [!WARNING]- Careful\n> x"), Some(Fold::Closed));
        assert_eq!(fold("> [!TODO]-\n> x"), Some(Fold::Closed));
        // The body does not carry the marker.
        assert_eq!(
            alert("> [!TIP]-\n> Hidden until opened."),
            Some((
                AlertKind::Tip,
                Fold::Closed,
                vec!["Hidden until opened.".to_string()]
            ))
        );
    }

    #[test]
    fn near_misses_stay_plain_quotes() {
        for src in [
            "> [!TODOS]\n> x",
            "> [!NOTE]x\n> y",
            "> [!BOGUS]-\n> x",
            "> text [!BUG]\n> x",
            "> `[!BUG]`\n> x",
            "> [!BUG\n> x",
            "> [! BUG]\n> x",
        ] {
            assert!(
                matches!(parse(src).first(), Some(Block::Quote(_))),
                "{src:?} must stay a quote"
            );
        }
    }

    #[test]
    fn an_alert_with_nothing_after_its_marker_has_no_body() {
        let got = alert("> [!TODO]-\n").expect("alert");
        assert_eq!((got.0, got.1), (AlertKind::Todo, Fold::Closed));
        assert!(got.2.is_empty());
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
        assert_eq!(
            text(spans),
            "Ship it 🎉 :unknown: see https://example.com/a, ok"
        );
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
