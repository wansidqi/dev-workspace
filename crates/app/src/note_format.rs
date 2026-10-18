//! Markdown formatting commands for the note editor toolbar.
//!
//! Pure functions over (text, selection) so they can be tested without GPUI.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Heading,
    InlineCode,
    CodeBlock,
    Table,
    Bullets,
    Numbered,
    Tasks,
    Quote,
    Link,
    Rule,
}

/// Replace `range` of the text with `text`, then select `select` (absolute
/// byte offsets into the *new* text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub select: Range<usize>,
}

/// Computes the edit for `format` applied to `selection` (byte offsets on
/// char boundaries; callers pass the editor's current selection).
pub fn apply(content: &str, selection: Range<usize>, format: Format) -> Edit {
    let selection = clamp(content, selection);
    match format {
        Format::Bold => wrap(content, selection, "**", "**", "bold"),
        Format::Italic => wrap(content, selection, "*", "*", "italic"),
        Format::InlineCode => wrap(content, selection, "`", "`", "code"),
        Format::Link => wrap(content, selection, "[", "](https://)", "link text"),
        Format::CodeBlock => code_block(content, selection),
        Format::Heading => line_prefix(content, selection, LinePrefix::Fixed("## ")),
        Format::Bullets => line_prefix(content, selection, LinePrefix::Fixed("- ")),
        Format::Tasks => line_prefix(content, selection, LinePrefix::Fixed("- [ ] ")),
        Format::Quote => line_prefix(content, selection, LinePrefix::Fixed("> ")),
        Format::Numbered => line_prefix(content, selection, LinePrefix::Numbered),
        Format::Table => insert_block(
            content,
            selection,
            "| Column | Column |\n| --- | --- |\n|  |  |",
        ),
        Format::Rule => insert_block(content, selection, "---"),
    }
}

fn clamp(content: &str, selection: Range<usize>) -> Range<usize> {
    let mut start = selection.start.min(content.len());
    let mut end = selection.end.min(content.len());
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    while !content.is_char_boundary(start) {
        start -= 1;
    }
    while !content.is_char_boundary(end) {
        end += 1;
    }
    start..end
}

fn wrap(content: &str, sel: Range<usize>, open: &str, close: &str, placeholder: &str) -> Edit {
    let inner = &content[sel.clone()];
    // Already wrapped exactly: unwrap instead (toggle).
    let before = &content[..sel.start];
    let after = &content[sel.end..];
    if !inner.is_empty() && before.ends_with(open) && after.starts_with(close) {
        let range = sel.start - open.len()..sel.end + close.len();
        let start = range.start;
        return Edit {
            range,
            text: inner.to_string(),
            select: start..start + inner.len(),
        };
    }
    let body = if inner.is_empty() { placeholder } else { inner };
    let text = format!("{open}{body}{close}");
    let start = sel.start + open.len();
    Edit {
        range: sel,
        text,
        select: start..start + body.len(),
    }
}

fn code_block(content: &str, sel: Range<usize>) -> Edit {
    let inner = &content[sel.clone()];
    let body = if inner.is_empty() { "code" } else { inner };
    // Fences must start on their own line.
    let lead = if sel.start == 0 || content[..sel.start].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let text = format!("{lead}```\n{body}\n```");
    let start = sel.start + lead.len() + 4;
    Edit {
        range: sel,
        text,
        select: start..start + body.len(),
    }
}

fn insert_block(content: &str, sel: Range<usize>, block: &str) -> Edit {
    let lead = if sel.start == 0 || content[..sel.start].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let text = format!("{lead}{block}\n");
    let end = sel.start + text.len();
    Edit {
        range: sel,
        text,
        select: end..end,
    }
}

enum LinePrefix {
    Fixed(&'static str),
    Numbered,
}

fn line_prefix(content: &str, sel: Range<usize>, prefix: LinePrefix) -> Edit {
    // Extend to whole lines.
    let start = content[..sel.start].rfind('\n').map_or(0, |i| i + 1);
    let end_probe = sel.end.max(sel.start);
    let end = content[end_probe..]
        .find('\n')
        .map_or(content.len(), |i| end_probe + i);
    let block = &content[start..end];
    let lines: Vec<&str> = block.split('\n').collect();

    let fixed = match &prefix {
        LinePrefix::Fixed(p) => Some(*p),
        LinePrefix::Numbered => None,
    };
    let numbered = |line: &str| -> Option<usize> {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        (digits > 0 && line[digits..].starts_with(". ")).then_some(digits + 2)
    };
    // Toggle off when every non-empty line already has the prefix.
    let all_have = lines.iter().filter(|l| !l.is_empty()).all(|l| match fixed {
        Some(p) => l.starts_with(p),
        None => numbered(l).is_some(),
    }) && lines.iter().any(|l| !l.is_empty());

    let mut number = 0;
    let rewritten: Vec<String> = lines
        .iter()
        .map(|line| {
            if all_have {
                match fixed {
                    Some(p) => line[p.len()..].to_string(),
                    None => line[numbered(line).unwrap_or(0)..].to_string(),
                }
            } else if line.is_empty() && lines.len() > 1 {
                String::new()
            } else {
                match fixed {
                    Some(p) => format!("{p}{line}"),
                    None => {
                        number += 1;
                        format!("{number}. {line}")
                    }
                }
            }
        })
        .collect();
    let text = rewritten.join("\n");
    let select = start..start + text.len();
    Edit {
        range: start..end,
        text,
        select,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(content: &str, sel: Range<usize>, f: Format) -> String {
        let edit = apply(content, sel, f);
        let mut out = content.to_string();
        out.replace_range(edit.range.clone(), &edit.text);
        assert!(out.is_char_boundary(edit.select.start));
        assert!(out.is_char_boundary(edit.select.end));
        out
    }

    #[test]
    fn bold_wraps_selection_and_toggles_off() {
        assert_eq!(run("say hi now", 4..6, Format::Bold), "say **hi** now");
        assert_eq!(run("say **hi** now", 6..8, Format::Bold), "say hi now");
    }

    #[test]
    fn empty_selection_inserts_a_placeholder() {
        let edit = apply("ab", 1..1, Format::Italic);
        assert_eq!(edit.text, "*italic*");
        assert_eq!(&edit.text[1..7], "italic");
    }

    #[test]
    fn headings_and_bullets_prefix_every_selected_line_and_toggle() {
        let text = "one\ntwo\nthree";
        assert_eq!(run(text, 0..7, Format::Bullets), "- one\n- two\nthree");
        assert_eq!(
            run("- one\n- two\nthree", 0..11, Format::Bullets),
            "one\ntwo\nthree"
        );
        assert_eq!(run("title", 2..2, Format::Heading), "## title");
    }

    #[test]
    fn numbered_lists_count_up() {
        assert_eq!(run("a\nb\nc", 0..5, Format::Numbered), "1. a\n2. b\n3. c");
        assert_eq!(run("1. a\n2. b", 0..9, Format::Numbered), "a\nb");
    }

    #[test]
    fn code_block_starts_on_its_own_line() {
        assert_eq!(run("x", 1..1, Format::CodeBlock), "x\n```\ncode\n```");
        assert_eq!(run("", 0..0, Format::CodeBlock), "```\ncode\n```");
        assert_eq!(
            run("a\nSELECT 1", 2..10, Format::CodeBlock),
            "a\n```\nSELECT 1\n```"
        );
    }

    #[test]
    fn table_and_rule_insert_at_cursor() {
        assert!(run("", 0..0, Format::Table).starts_with("| Column | Column |"));
        assert_eq!(run("a", 1..1, Format::Rule), "a\n---\n");
    }

    #[test]
    fn multibyte_text_and_out_of_range_selections_do_not_panic() {
        assert_eq!(run("héllo", 0..3, Format::Bold), "**hé**llo");
        let _ = apply("héllo", 2..99, Format::Quote);
        let _ = apply("", 5..9, Format::Link);
        let _ = apply("日本語", 1..2, Format::InlineCode);
    }
}
