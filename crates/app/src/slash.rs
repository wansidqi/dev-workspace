//! Slash commands for the note editor: `/` at the start of a line opens a list
//! of blocks to insert (table, code, alert, task list and so on). Pure, so the
//! trigger and matching rules are unit tested here; the editor draws the popup
//! and applies the chosen entry.

use crate::note_format::Format;

/// What picking an entry does to the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The same edit the formatting toolbar makes for this format.
    Format(Format),
    /// Inserts fixed text and puts the caret `caret` bytes into it.
    Insert { text: &'static str, caret: usize },
}

/// One command in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// What is typed after the `/`, and what the list shows beside the title.
    pub name: &'static str,
    pub title: &'static str,
    /// Other words that find it (`alert` finds every callout).
    pub keywords: &'static [&'static str],
    pub action: Action,
}

/// A callout in GitHub's alert syntax, with the caret on the empty line inside.
macro_rules! alert {
    ($name:literal, $title:literal, $kind:literal) => {
        Entry {
            name: $name,
            title: $title,
            keywords: &["alert", "callout"],
            action: Action::Insert {
                text: concat!("> [!", $kind, "]\n> "),
                caret: concat!("> [!", $kind, "]\n> ").len(),
            },
        }
    };
}

/// Every command, most used first (the order of an empty query).
pub const ENTRIES: &[Entry] = &[
    Entry {
        name: "heading",
        title: "Heading",
        keywords: &["title", "h1"],
        action: Action::Format(Format::Heading),
    },
    Entry {
        name: "table",
        title: "Table",
        keywords: &["grid"],
        action: Action::Format(Format::Table),
    },
    Entry {
        name: "code",
        title: "Code block",
        keywords: &["fence", "snippet"],
        action: Action::Format(Format::CodeBlock),
    },
    Entry {
        name: "sql",
        title: "SQL block",
        keywords: &["query", "database"],
        action: Action::Format(Format::SqlBlock),
    },
    Entry {
        name: "tasks",
        title: "Task list",
        keywords: &["todo", "checklist", "checkbox"],
        action: Action::Format(Format::Tasks),
    },
    Entry {
        name: "bullets",
        title: "Bulleted list",
        keywords: &["list", "ul"],
        action: Action::Format(Format::Bullets),
    },
    Entry {
        name: "numbered",
        title: "Numbered list",
        keywords: &["list", "ol", "steps"],
        action: Action::Format(Format::Numbered),
    },
    Entry {
        name: "quote",
        title: "Quote",
        keywords: &["blockquote"],
        action: Action::Format(Format::Quote),
    },
    alert!("note", "Note callout", "NOTE"),
    alert!("tip", "Tip callout", "TIP"),
    alert!("important", "Important callout", "IMPORTANT"),
    alert!("warning", "Warning callout", "WARNING"),
    alert!("caution", "Caution callout", "CAUTION"),
    Entry {
        name: "link",
        title: "Link",
        keywords: &["url"],
        action: Action::Format(Format::Link),
    },
    Entry {
        name: "divider",
        title: "Divider",
        keywords: &["rule", "line", "hr"],
        action: Action::Format(Format::Rule),
    },
];

/// Rows the popup shows.
pub const MAX_ENTRIES: usize = 10;
/// Longest command word the popup follows.
const MAX_QUERY: usize = 24;

/// The entry called `name`.
pub fn find(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.name == name)
}

/// When the caret is in a `/word` that starts its line: the byte offset of the
/// `/` and the word typed after it. The word may hold letters, digits and `-`
/// only, so a path (`/usr/bin`) or a sentence never keeps it open. Inside a
/// fenced code block it never opens, since paths and comments start lines there.
pub fn open_query(content: &str, caret: usize) -> Option<(usize, &str)> {
    let before = content.get(..caret)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let query = before[line_start..].strip_prefix('/')?;
    if query.len() > MAX_QUERY || !query.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    if in_code_fence(&content[..line_start]) {
        return None;
    }
    Some((line_start, query))
}

/// The note text after the typed `/word` (`start..caret`) is replaced by what
/// `entry` makes, and where the selection goes. A format gets the edit the
/// toolbar would make on the emptied line, so the two always agree.
pub fn apply(
    content: &str,
    start: usize,
    caret: usize,
    entry: &Entry,
) -> (String, std::ops::Range<usize>) {
    let mut text = content.to_string();
    text.replace_range(start..caret, "");
    let edit = match entry.action {
        Action::Format(format) => {
            let mut edit = crate::note_format::apply(&text, start..start, format);
            // The toolbar leaves a line marker (`- `, `# `, `> `) selected so a
            // second click can toggle it off; here it would be overwritten by
            // the first letter typed, so the caret goes after it instead.
            if matches!(
                format,
                Format::Heading
                    | Format::Bullets
                    | Format::Numbered
                    | Format::Tasks
                    | Format::Quote
            ) {
                edit.select = edit.select.end..edit.select.end;
            }
            edit
        }
        Action::Insert { text: block, caret } => crate::note_format::Edit {
            range: start..start,
            text: block.to_string(),
            select: start + caret..start + caret,
        },
    };
    text.replace_range(edit.range, &edit.text);
    (text, edit.select)
}

/// Whether the text ends inside an open fenced code block: an odd number of
/// lines that start with a fence.
fn in_code_fence(before: &str) -> bool {
    before
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("```") || line.starts_with("~~~")
        })
        .count()
        % 2
        == 1
}

/// The commands that match `query`, best first: a name that starts with it,
/// then a keyword or title word that starts with it, then a name or title that
/// contains it. Case is ignored; an empty query lists them in the given order.
pub fn filter(query: &str) -> Vec<&'static Entry> {
    let q = query.trim().to_lowercase();
    let word_start = |text: &str| {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w.starts_with(&q))
    };
    let mut scored: Vec<(u8, &'static Entry)> = ENTRIES
        .iter()
        .filter_map(|e| {
            let score = if q.is_empty() || e.name.starts_with(&q) {
                0
            } else if word_start(e.title) || e.keywords.iter().any(|k| k.starts_with(&q)) {
                1
            } else if e.name.contains(&q) || e.title.to_lowercase().contains(&q) {
                2
            } else {
                return None;
            };
            Some((score, e))
        })
        .collect();
    scored.sort_by_key(|(score, _)| *score);
    scored
        .into_iter()
        .take(MAX_ENTRIES)
        .map(|(_, e)| e)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_on_a_slash_that_starts_the_line() {
        assert_eq!(open_query("/", 1), Some((0, "")));
        assert_eq!(open_query("/ta", 3), Some((0, "ta")));
        assert_eq!(open_query("intro\n/code", 11), Some((6, "code")));
        assert_eq!(open_query("/table-x", 8), Some((0, "table-x")));
    }

    #[test]
    fn stays_closed_for_paths_urls_and_mid_line_slashes() {
        assert_eq!(open_query("/usr/bin", 8), None);
        assert_eq!(open_query("see /table", 10), None);
        assert_eq!(
            open_query("  /table", 8),
            None,
            "indented text is not a command"
        );
        assert_eq!(open_query("https://x.test", 14), None);
        assert_eq!(open_query("/table now", 10), None, "a space ends the word");
        assert_eq!(open_query("/a_b", 4), None);
        assert_eq!(
            open_query("/abcdefghijklmnopqrstuvwxyz", 27),
            None,
            "too long"
        );
        assert_eq!(open_query("é/x", "é/x".len()), None);
    }

    #[test]
    fn stays_closed_inside_a_code_fence() {
        let note = "text\n```sh\n/etc/hosts\n```\n/ta";
        // Inside the fence.
        assert_eq!(open_query(note, "text\n```sh\n/et".len()), None);
        // After the fence is closed it works again.
        assert_eq!(open_query(note, note.len()), Some((note.len() - 3, "ta")));
        // An unclosed fence keeps everything after it closed.
        assert_eq!(open_query("```\n/t", 6), None);
        assert_eq!(open_query("~~~\n/t", 6), None);
    }

    #[test]
    fn matches_by_name_then_keyword_then_contains() {
        let names = |q: &str| -> Vec<&str> { filter(q).into_iter().map(|e| e.name).collect() };
        assert_eq!(names("ta")[0], "table", "name prefix first");
        assert!(names("ta").contains(&"tasks"));
        // A keyword finds the entry: `todo` is not a name.
        assert_eq!(names("todo"), ["tasks"]);
        // `alert` finds every callout.
        assert_eq!(
            names("alert"),
            ["note", "tip", "important", "warning", "caution"]
        );
        // Contains: `ing` starts no word but sits inside `warning` and `heading`.
        assert!(names("ing").contains(&"warning"));
        assert!(names("ing").contains(&"heading"));
        assert!(names("zzz").is_empty());
        assert_eq!(names("TABLE"), ["table"]);
    }

    #[test]
    fn an_empty_query_lists_the_most_used_first_within_the_cap() {
        let all = filter("");
        assert_eq!(all.len(), MAX_ENTRIES);
        assert_eq!(all[0].name, "heading");
        assert!(ENTRIES.len() > MAX_ENTRIES, "typing narrows the rest in");
    }

    #[test]
    fn a_command_replaces_the_typed_word_with_its_block() {
        let run = |note: &str, name: &str| {
            let (start, query) = open_query(note, note.len()).expect("a slash command");
            let entry = find(name).expect("known command");
            assert_eq!(start + 1 + query.len(), note.len());
            apply(note, start, note.len(), entry)
        };
        // Text before the line is kept and the typed `/word` is gone.
        let (text, _) = run("intro\n\n/table", "table");
        assert!(text.starts_with("intro\n\n"));
        assert!(!text.contains("/table"));
        assert!(text.contains("| --- |") || text.contains("---"), "{text:?}");
        // A list command leaves the caret after its marker, on the same line.
        let (text, sel) = run("/bullets", "bullets");
        assert_eq!(text, "- ");
        assert_eq!(sel, 2..2, "collapsed after the marker, not selecting it");
        for name in ["heading", "numbered", "tasks", "quote"] {
            let (text, sel) = run(&format!("/{name}"), name);
            assert!(
                sel.is_empty() && sel.end == text.len(),
                "{name}: {text:?} {sel:?}"
            );
        }
        // A code block is fenced and the caret sits inside it.
        let (text, sel) = run("/code", "code");
        assert!(text.contains("```"));
        assert!(sel.start > 0 && sel.start < text.len());
        // A callout ends with the caret on the empty quoted line.
        let (text, sel) = run("/warning", "warning");
        assert_eq!(text, "> [!WARNING]\n> ");
        assert_eq!(sel, text.len()..text.len());
    }

    #[test]
    fn every_command_applies_without_panicking_anywhere_in_a_note() {
        let notes = ["", "/x", "a\n/x", "a\n\n/x\nb", "é\n/x\n\nz"];
        for note in notes {
            let caret = note.find("/x").map(|i| i + 2);
            let Some(caret) = caret else { continue };
            let start = caret - 2;
            for entry in ENTRIES {
                let (text, sel) = apply(note, start, caret, entry);
                assert!(sel.end <= text.len(), "{} on {note:?}", entry.name);
                assert!(text.is_char_boundary(sel.start) && text.is_char_boundary(sel.end));
            }
        }
    }

    #[test]
    fn names_are_unique_and_callouts_are_valid_alerts() {
        for (i, e) in ENTRIES.iter().enumerate() {
            assert!(
                ENTRIES[i + 1..].iter().all(|o| o.name != e.name),
                "duplicate name {}",
                e.name
            );
            assert!(
                e.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{} could never be typed after the slash",
                e.name
            );
            assert_eq!(find(e.name), Some(e));
            if let Action::Insert { text, caret } = e.action {
                assert!(text.starts_with("> [!") && text.contains("]\n> "));
                assert_eq!(caret, text.len(), "caret goes on the empty quoted line");
            }
        }
        assert_eq!(find("nope"), None);
    }
}
