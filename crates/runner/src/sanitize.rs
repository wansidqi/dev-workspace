//! Neutralises process output before it reaches the UI: ANSI escape
//! sequences and control characters must not be able to corrupt the view
//! or spoof content, and lines are length-capped.

/// Cleans one line of raw output: strips ANSI escape sequences (CSI, OSC and
/// two-byte escapes), turns tabs into spaces, drops every other control
/// character and bidirectional-override characters, and caps the length at
/// `max_chars` (adding an ellipsis when cut).
pub fn sanitize_line(raw: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_chars + 4));
    let mut count = 0usize;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek().copied() {
                // CSI: ESC [ params... final byte 0x40..=0x7E
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ESC \
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Any other ESC <char> pair.
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        let c = if c == '\t' { ' ' } else { c };
        if c.is_control() || is_bidi_control(c) {
            continue;
        }
        if count >= max_chars {
            out.push('\u{2026}');
            return out;
        }
        out.push(c);
        count += 1;
    }
    out
}

/// Bidi embedding/override/isolate characters can visually reorder text.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_colours_and_osc_titles() {
        assert_eq!(sanitize_line("\u{1b}[31mred\u{1b}[0m ok", 100), "red ok");
        assert_eq!(sanitize_line("\u{1b}]0;title\u{7}text", 100), "text");
        assert_eq!(sanitize_line("\u{1b}]8;;http://x\u{1b}\\link", 100), "link");
        assert_eq!(sanitize_line("a\u{1b}[2Jb\u{1b}[10;5Hc", 100), "abc");
    }

    #[test]
    fn drops_control_chars_but_keeps_text() {
        assert_eq!(sanitize_line("a\u{7}b\u{0}c\u{8}d\u{7f}e", 100), "abcde");
        assert_eq!(sanitize_line("a\tb", 100), "a b");
        assert_eq!(sanitize_line("x\u{202e}y\u{85}z", 100), "xyz");
        assert_eq!(sanitize_line("héllo wörld ✓", 100), "héllo wörld ✓");
    }

    #[test]
    fn caps_length_with_an_ellipsis() {
        let long = "x".repeat(50);
        let out = sanitize_line(&long, 10);
        assert_eq!(out.chars().count(), 11);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(sanitize_line("short", 10), "short");
    }

    #[test]
    fn a_dangling_escape_does_not_panic() {
        assert_eq!(sanitize_line("abc\u{1b}", 10), "abc");
        assert_eq!(sanitize_line("abc\u{1b}[31", 10), "abc");
    }
}
