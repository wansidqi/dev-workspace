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

#[cfg(test)]
mod tests {
    use super::*;

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
