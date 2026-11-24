//! Word navigation shared by the single-line input and the note editor
//! (Ctrl+Left/Right, Ctrl+Shift+Left/Right, Ctrl+Backspace/Delete). Offsets
//! are byte offsets into `text` and always land on character boundaries.

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Other
    }
}

/// Start of the word before `offset`: skips spaces, then one run of the same
/// kind of character (letters/digits, or punctuation).
pub fn word_start_before(text: &str, offset: usize) -> usize {
    let mut chars = text[..offset.min(text.len())]
        .char_indices()
        .rev()
        .peekable();
    while chars.next_if(|(_, c)| class(*c) == Class::Space).is_some() {}
    let Some(&(mut start, first)) = chars.peek() else {
        return 0;
    };
    let kind = class(first);
    while let Some((i, _)) = chars.next_if(|(_, c)| class(*c) == kind) {
        start = i;
    }
    start
}

/// Start of the next word after `offset`: skips the rest of the current run,
/// then the spaces after it.
pub fn word_end_after(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut chars = text[offset..].char_indices().peekable();
    if let Some(&(_, first)) = chars.peek() {
        let kind = class(first);
        if kind != Class::Space {
            while chars.next_if(|(_, c)| class(*c) == kind).is_some() {}
        }
    }
    while chars.next_if(|(_, c)| class(*c) == Class::Space).is_some() {}
    chars.peek().map_or(text.len(), |(i, _)| offset + i)
}

/// Most matches the find bar tracks; a one-letter query in a huge note would
/// otherwise build a list nobody can step through.
pub const MAX_FIND_MATCHES: usize = 5000;

/// Every non-overlapping occurrence of `query` in `text`, as byte ranges,
/// ignoring ASCII case (so offsets always line up with `text`). An empty query
/// matches nothing.
pub fn find_all(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let haystack = text.to_ascii_lowercase();
    let needle = query.to_ascii_lowercase();
    haystack
        .match_indices(&needle)
        .take(MAX_FIND_MATCHES)
        .map(|(i, m)| i..i + m.len())
        .collect()
}

/// The run of letters/digits (or of punctuation, or of spaces) that `offset`
/// touches, for double-click selection. Prefers the character after the
/// offset; at the end of the text or before a space it falls back to the
/// character before, so clicking just past a word still selects it.
pub fn word_at(text: &str, offset: usize) -> std::ops::Range<usize> {
    let offset = offset.min(text.len());
    let after = text[offset..].chars().next().map(class);
    let before = text[..offset].chars().next_back().map(class);
    let kind = match (before, after) {
        (Some(b), Some(Class::Space)) if b != Class::Space => b,
        (_, Some(a)) => a,
        (Some(b), None) => b,
        (None, None) => return offset..offset,
    };
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| class(*c) != kind)
        .map_or(text.len(), |(i, _)| offset + i);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_all_ignores_ascii_case_and_keeps_byte_offsets() {
        assert_eq!(find_all("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
        assert!(find_all("abc", "").is_empty());
        assert!(find_all("abc", "z").is_empty());
        let t = "héllo Hello";
        let hits = find_all(t, "hello");
        assert_eq!(hits.len(), 1);
        assert_eq!(&t[hits[0].clone()], "Hello");
        assert_eq!(find_all("aaaa", "aa"), vec![0..2, 2..4]);
    }

    #[test]
    fn word_at_selects_the_touched_run() {
        let t = "foo bar.baz";
        assert_eq!(word_at(t, 5), 4..7);
        assert_eq!(word_at(t, 4), 4..7);
        assert_eq!(word_at(t, 3), 0..3); // just past "foo"
        assert_eq!(word_at(t, 7), 7..8); // the dot
        assert_eq!(word_at(t, t.len()), 8..11);
        assert_eq!(word_at("", 0), 0..0);
        let m = "héllo 日本語";
        let r = word_at(m, 3);
        assert_eq!(&m[r], "héllo");
    }

    #[test]
    fn moves_back_over_words_and_punctuation() {
        let t = "foo bar.baz  qux";
        assert_eq!(word_start_before(t, t.len()), 13);
        assert_eq!(word_start_before(t, 13), 8);
        assert_eq!(word_start_before(t, 8), 7);
        assert_eq!(word_start_before(t, 7), 4);
        assert_eq!(word_start_before(t, 4), 0);
        assert_eq!(word_start_before(t, 0), 0);
    }

    #[test]
    fn moves_forward_to_the_next_word_start() {
        let t = "foo bar.baz  qux";
        assert_eq!(word_end_after(t, 0), 4);
        assert_eq!(word_end_after(t, 4), 7);
        assert_eq!(word_end_after(t, 7), 8);
        assert_eq!(word_end_after(t, 8), 13);
        assert_eq!(word_end_after(t, 13), t.len());
        assert_eq!(word_end_after(t, t.len()), t.len());
    }

    #[test]
    fn multibyte_text_lands_on_boundaries() {
        let t = "héllo 日本語 x";
        let a = word_start_before(t, t.len());
        assert!(t.is_char_boundary(a));
        let b = word_end_after(t, 0);
        assert!(t.is_char_boundary(b));
        assert_eq!(&t[b..], "日本語 x");
    }
}
