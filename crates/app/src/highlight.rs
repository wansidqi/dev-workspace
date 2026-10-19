//! A small, dependency-free syntax tokenizer for fenced code blocks in the
//! note preview. It colours comments, strings, numbers, keywords and
//! function calls well enough to read SQL and common scripts; it is not a
//! parser, and unknown languages only get strings, numbers and comments.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Keyword,
    String,
    Number,
    Comment,
    Function,
}

const SQL_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "in",
    "is",
    "null",
    "as",
    "on",
    "join",
    "inner",
    "left",
    "right",
    "outer",
    "cross",
    "full",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "union",
    "all",
    "distinct",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "alter",
    "drop",
    "table",
    "index",
    "view",
    "database",
    "show",
    "full",
    "processlist",
    "like",
    "between",
    "exists",
    "case",
    "when",
    "then",
    "else",
    "end",
    "asc",
    "desc",
    "with",
    "explain",
    "use",
    "force",
    "primary",
    "key",
    "foreign",
    "references",
    "default",
    "if",
    "begin",
    "commit",
    "rollback",
    "truncate",
    "describe",
    "grant",
    "revoke",
    "count",
    "sum",
    "avg",
    "min",
    "max",
];

const CODE_KEYWORDS: &[&str] = &[
    "fn",
    "let",
    "mut",
    "const",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "return",
    "match",
    "pub",
    "use",
    "mod",
    "struct",
    "enum",
    "impl",
    "trait",
    "async",
    "await",
    "function",
    "var",
    "class",
    "import",
    "from",
    "export",
    "default",
    "new",
    "this",
    "true",
    "false",
    "null",
    "undefined",
    "def",
    "elif",
    "in",
    "not",
    "and",
    "or",
    "None",
    "True",
    "False",
    "then",
    "fi",
    "do",
    "done",
    "echo",
    "cd",
    "sudo",
    "git",
    "npm",
    "cargo",
];

/// Languages whose line comments start with `#`.
fn hash_comments(lang: &str) -> bool {
    matches!(
        lang,
        "sh" | "bash" | "zsh" | "shell" | "py" | "python" | "yaml" | "yml" | "toml" | "rb" | "ruby"
    )
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Tokens of `text` as byte ranges, in order and non-overlapping.
pub fn tokenize(lang: &str, text: &str) -> Vec<(Range<usize>, Token)> {
    let lang = lang.trim().to_ascii_lowercase();
    let is_sql = matches!(
        lang.as_str(),
        "sql" | "mysql" | "postgres" | "postgresql" | "sqlite"
    );
    let keywords: &[&str] = if is_sql {
        SQL_KEYWORDS
    } else if lang.is_empty() {
        &[]
    } else {
        CODE_KEYWORDS
    };
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let end_of = |i: usize| chars.get(i).map_or(text.len(), |c| c.0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        let next = chars.get(i + 1).map(|c| c.1);
        // Comments.
        let hash = c == '#' && (is_sql || hash_comments(&lang));
        let slashes =
            !is_sql && !hash_comments(&lang) && !lang.is_empty() && c == '/' && next == Some('/');
        let dashes = is_sql && c == '-' && next == Some('-');
        let line_comment = hash || slashes || dashes;
        if line_comment {
            let mut j = i;
            while j < chars.len() && chars[j].1 != '\n' {
                j += 1;
            }
            out.push((start..end_of(j), Token::Comment));
            i = j;
            continue;
        }
        if c == '/' && next == Some('*') {
            let mut j = i + 2;
            while j < chars.len()
                && !(chars[j].1 == '*' && chars.get(j + 1).map(|c| c.1) == Some('/'))
            {
                j += 1;
            }
            let j = (j + 2).min(chars.len());
            out.push((start..end_of(j), Token::Comment));
            i = j;
            continue;
        }
        // Strings (a backslash escapes the next character; SQL also doubles quotes).
        if c == '\'' || c == '"' || (c == '`' && !is_sql) {
            let mut j = i + 1;
            while j < chars.len() && chars[j].1 != c {
                if chars[j].1 == '\\' {
                    j += 1;
                }
                j += 1;
            }
            let j = (j + 1).min(chars.len());
            out.push((start..end_of(j), Token::String));
            i = j;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '.') {
                j += 1;
            }
            out.push((start..end_of(j), Token::Number));
            i = j;
            continue;
        }
        if is_ident_start(c) {
            let mut j = i;
            while j < chars.len() && is_ident(chars[j].1) {
                j += 1;
            }
            let word = &text[start..end_of(j)];
            let called = chars.get(j).map(|c| c.1) == Some('(');
            if keywords.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                let kind = if called
                    && is_sql
                    && matches!(
                        word.to_ascii_lowercase().as_str(),
                        "count" | "sum" | "avg" | "min" | "max"
                    ) {
                    Token::Function
                } else {
                    Token::Keyword
                };
                out.push((start..end_of(j), kind));
            } else if called {
                out.push((start..end_of(j), Token::Function));
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, text: &str) -> Vec<(String, Token)> {
        tokenize(lang, text)
            .into_iter()
            .map(|(r, t)| (text[r].to_string(), t))
            .collect()
    }

    #[test]
    fn sql_keywords_strings_numbers_and_comments() {
        let got = kinds("sql", "SELECT id FROM t WHERE x = 'a' -- note\nLIMIT 10");
        assert!(got.contains(&("SELECT".into(), Token::Keyword)));
        assert!(got.contains(&("'a'".into(), Token::String)));
        assert!(got.contains(&("-- note".into(), Token::Comment)));
        assert!(got.contains(&("10".into(), Token::Number)));
        assert!(!got.iter().any(|(w, _)| w == "id"));
    }

    #[test]
    fn function_calls_are_marked_and_block_comments_span_lines() {
        let got = kinds("sql", "SUM(a) /* x\ny */ IFNULL(b, 0)");
        assert!(got.contains(&("SUM".into(), Token::Function)));
        assert!(got.contains(&("IFNULL".into(), Token::Function)));
        assert!(got.contains(&("/* x\ny */".into(), Token::Comment)));
    }

    #[test]
    fn unknown_languages_only_get_strings_and_numbers() {
        let got = kinds("", "select 1 'x'");
        assert_eq!(
            got,
            vec![("1".into(), Token::Number), ("'x'".into(), Token::String)]
        );
    }

    #[test]
    fn unterminated_strings_and_multibyte_text_do_not_panic() {
        let _ = tokenize("sql", "SELECT 'héllo");
        let _ = tokenize("js", "const s = \"日本\\");
        let _ = tokenize("sql", "/* open");
    }
}
