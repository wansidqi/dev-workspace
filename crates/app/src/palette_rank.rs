//! Pure ranking and result assembly for the command palette.
//!
//! Kept free of GPUI so it is unit-testable (CLAUDE.md: logic stays out of
//! views). Everything searched here (titles, commit messages) is untrusted
//! text that is only compared and displayed, never interpreted.

use devspace_core::{NoteId, ProjectId, TaskId};

/// Hard cap on rows shown, so the list stays cheap to build and render.
pub const MAX_RESULTS: usize = 50;
/// Queries longer than this are truncated; nobody types a longer palette
/// query, and the cap bounds the fuzzy matcher's work.
const MAX_QUERY_CHARS: usize = 128;
/// Candidate text beyond this is ignored for the same reason.
const MAX_TEXT_CHARS: usize = 512;

// Tier bases are 1000 apart and every in-tier penalty stays below 1000, so
// a better tier always beats a worse one regardless of position or length.
const TIER_EXACT: u32 = 5000;
const TIER_PREFIX: u32 = 4000;
const TIER_WORD_PREFIX: u32 = 3000;
const TIER_SUBSTRING: u32 = 2000;
const TIER_FUZZY: u32 = 1000;

fn fold(s: &str, max_chars: usize) -> Vec<char> {
    s.chars()
        .take(max_chars)
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Smaller penalty for tighter matches: slightly prefers shorter candidates
/// within a tier so "build" ranks "Build" above "Build and deploy".
fn len_penalty(extra: usize) -> u32 {
    u32::try_from(extra.min(400)).unwrap_or(400)
}

fn find(text: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > text.len() {
        return None;
    }
    (0..=text.len() - needle.len()).find(|&i| text[i..i + needle.len()] == *needle)
}

fn is_word_start(text: &[char], i: usize) -> bool {
    i == 0 || !text[i - 1].is_alphanumeric()
}

fn find_word_prefix(text: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > text.len() {
        return None;
    }
    (0..=text.len() - needle.len())
        .find(|&i| is_word_start(text, i) && text[i..i + needle.len()] == *needle)
}

/// Greedy subsequence match; returns the span covered so tighter matches
/// score higher.
fn fuzzy_span(text: &[char], needle: &[char]) -> Option<usize> {
    let mut start = None;
    let mut ni = 0;
    for (ti, &c) in text.iter().enumerate() {
        if ni < needle.len() && c == needle[ni] {
            start.get_or_insert(ti);
            ni += 1;
            if ni == needle.len() {
                return Some(ti + 1 - start.unwrap_or(ti));
            }
        }
    }
    None
}

fn tiered(q: &[char], t: &[char], allow_fuzzy: bool) -> Option<u32> {
    if q.is_empty() {
        return None;
    }
    if q == t {
        return Some(TIER_EXACT);
    }
    let extra = t.len().saturating_sub(q.len());
    if t.starts_with(q) {
        return Some(TIER_PREFIX - len_penalty(extra));
    }
    if let Some(i) = find_word_prefix(t, q) {
        return Some(TIER_WORD_PREFIX - len_penalty(i * 2 + extra));
    }
    if let Some(i) = find(t, q) {
        return Some(TIER_SUBSTRING - len_penalty(i * 2 + extra));
    }
    if allow_fuzzy {
        let span = fuzzy_span(t, q)?;
        return Some(TIER_FUZZY - len_penalty((span - q.len()) * 4 + extra));
    }
    None
}

/// Scores how well `text` matches `query`, higher is better, `None` when it
/// does not match. Case-insensitive and Unicode-safe (works on chars, so no
/// byte-boundary panics). Tiers: exact > prefix > word-prefix > substring >
/// fuzzy subsequence. A multi-word query that is not a contiguous phrase
/// matches when every word is at least a substring. A blank query matches
/// everything with score 0.
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    let q = fold(query, MAX_QUERY_CHARS);
    let t = fold(text, MAX_TEXT_CHARS);
    let words: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|w| fold(w, MAX_QUERY_CHARS))
        .collect();
    if words.len() <= 1 {
        return tiered(&q, &t, true);
    }
    if let Some(s) = tiered(&q, &t, false) {
        return Some(s);
    }
    // Words in any order: the weakest word decides, minus a small penalty
    // so a contiguous phrase always outranks scattered words.
    let mut worst = u32::MAX;
    for w in &words {
        worst = worst.min(tiered(w, &t, false)?);
    }
    Some(worst.saturating_sub(100))
}

/// Indices of `items` that match `query`, best first, at most `limit`. Ties
/// keep the input order (stable), so callers can pre-sort by recency. A
/// blank query returns the first `limit` items in input order.
pub fn rank<T>(query: &str, items: &[T], text_of: impl Fn(&T) -> &str, limit: usize) -> Vec<usize> {
    let mut scored: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| score(query, text_of(item)).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, i)| i).collect()
}

/// Section a result belongs to; the declaration order is the display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Commands,
    Projects,
    Tasks,
    Notes,
    /// Notes whose text (not title) matched; filtered by the file search.
    NoteText,
    Commits,
}

impl Group {
    /// Small heading shown above the group's rows.
    pub fn label(self) -> &'static str {
        match self {
            Group::Commands => "COMMANDS",
            Group::Projects => "PROJECTS",
            Group::Tasks => "TASKS",
            Group::Notes => "NOTES",
            Group::NoteText => "IN NOTES",
            Group::Commits => "COMMITS",
        }
    }

    /// Most rows one group may contribute, so no source can crowd out the
    /// others.
    fn cap(self, query_blank: bool) -> usize {
        match (self, query_blank) {
            (Group::Commands, _) => 12,
            (Group::Projects, true) => 5,
            (Group::Projects, false) => 6,
            (Group::Tasks, true) | (Group::Notes, true) => 5,
            (Group::Tasks, false) | (Group::Notes, false) => 12,
            (Group::NoteText, _) => 8,
            (Group::Commits, _) => 10,
        }
    }
}

/// Built-in commands the palette can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ToggleTerminal,
    NewTerminal,
    OpenHistory,
    OpenRunCommands,
    OpenTimesheet,
    OpenCredentials,
    OpenSettings,
    NewProject,
    CycleTheme,
    ToggleReduceTransparency,
    ToggleReduceMotion,
}

impl Command {
    /// All commands, in the order shown for an empty query.
    pub const ALL: [Command; 11] = [
        Command::ToggleTerminal,
        Command::NewTerminal,
        Command::OpenHistory,
        Command::OpenRunCommands,
        Command::OpenTimesheet,
        Command::OpenCredentials,
        Command::OpenSettings,
        Command::NewProject,
        Command::CycleTheme,
        Command::ToggleReduceTransparency,
        Command::ToggleReduceMotion,
    ];

    /// Human title, also what the query is matched against.
    pub fn title(self) -> &'static str {
        match self {
            Command::ToggleTerminal => "Show/Hide terminal",
            Command::NewTerminal => "New terminal",
            Command::OpenHistory => "Open commit history",
            Command::OpenRunCommands => "Open run commands",
            Command::OpenTimesheet => "Open timesheet",
            Command::OpenCredentials => "Open credentials",
            Command::OpenSettings => "Open settings",
            Command::NewProject => "New project",
            Command::CycleTheme => "Switch theme (system, dark, light)",
            Command::ToggleReduceTransparency => "Toggle reduced transparency",
            Command::ToggleReduceMotion => "Toggle reduced motion",
        }
    }

    /// Shortcut hint shown on the row, when there is one.
    pub fn hint(self) -> &'static str {
        match self {
            Command::ToggleTerminal => "Ctrl+`",
            Command::NewTerminal => "Ctrl+Shift+`",
            Command::OpenHistory => "Ctrl+Shift+H",
            Command::OpenRunCommands => "Ctrl+Shift+R",
            Command::OpenTimesheet => "Ctrl+Shift+M",
            Command::OpenCredentials => "Ctrl+Shift+L",
            Command::OpenSettings => "Ctrl+,",
            Command::NewProject => "Ctrl+Shift+N",
            Command::CycleTheme
            | Command::ToggleReduceTransparency
            | Command::ToggleReduceMotion => "",
        }
    }
}

/// What running a row does. Carries the ids needed to navigate, including
/// the owning project so a hit in another project can switch to it first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Command(Command),
    Project(ProjectId),
    Task {
        project: ProjectId,
        task: TaskId,
    },
    Note {
        project: ProjectId,
        note: NoteId,
        task: Option<TaskId>,
        title: String,
    },
    /// A commit of the selected project; opens the commit browser.
    Commit,
}

/// One row of the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub group: Group,
    pub title: String,
    /// Dim secondary text (project name, repo and hash, shortcut).
    pub detail: String,
    pub target: Target,
}

/// Candidates per source, each already in "most relevant first" order for a
/// blank query (recency). Commits arrive pre-filtered by git and are kept in
/// that (newest-first) order.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub commands: Vec<Item>,
    pub projects: Vec<Item>,
    pub tasks: Vec<Item>,
    pub notes: Vec<Item>,
    /// Notes found by their text; already filtered, kept in the given order.
    pub note_text: Vec<Item>,
    pub commits: Vec<Item>,
}

impl Sources {
    /// The built-in command rows.
    pub fn command_items() -> Vec<Item> {
        Command::ALL
            .iter()
            .map(|&c| Item {
                group: Group::Commands,
                title: c.title().to_string(),
                detail: c.hint().to_string(),
                target: Target::Command(c),
            })
            .collect()
    }
}

/// Ranks each source against `query` and concatenates the groups in display
/// order, capped per group and in total. A blank query shows commands plus
/// recent projects/tasks/notes only (no commits).
pub fn assemble(query: &str, sources: Sources) -> Vec<Item> {
    let blank = query.trim().is_empty();
    let mut out: Vec<Item> = Vec::new();
    let groups = [
        (Group::Commands, sources.commands),
        (Group::Projects, sources.projects),
        (Group::Tasks, sources.tasks),
        (Group::Notes, sources.notes),
        (
            Group::NoteText,
            if blank { Vec::new() } else { sources.note_text },
        ),
        (
            Group::Commits,
            if blank { Vec::new() } else { sources.commits },
        ),
    ];
    for (group, items) in groups {
        let cap = group.cap(blank);
        let order = if matches!(group, Group::NoteText | Group::Commits) {
            // The file search and git already filtered and ordered these.
            (0..items.len()).take(cap).collect()
        } else {
            rank(query, &items, |i| i.title.as_str(), cap)
        };
        for i in order {
            if out.len() >= MAX_RESULTS {
                return out;
            }
            out.push(items[i].clone());
        }
    }
    out
}

/// Next highlight index when moving by `delta` (+1 / -1) in a list of
/// `len` rows, wrapping around; 0 for an empty list.
pub fn step_highlight(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    (current as isize + delta).rem_euclid(len) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, t: &str) -> Option<u32> {
        score(q, t)
    }

    #[test]
    fn tiers_are_ordered() {
        let exact = s("build", "Build").unwrap();
        let prefix = s("build", "Build and deploy").unwrap();
        let word = s("build", "Run build").unwrap();
        let sub = s("build", "Rebuilder").unwrap();
        let fuzzy = s("bld", "Build").unwrap();
        assert!(exact > prefix, "{exact} {prefix}");
        assert!(prefix > word, "{prefix} {word}");
        assert!(word > sub, "{word} {sub}");
        assert!(sub > fuzzy, "{sub} {fuzzy}");
        assert!(fuzzy > 0);
    }

    #[test]
    fn long_text_never_outranks_a_better_tier() {
        let long_prefix = format!("build {}", "x".repeat(500));
        let short_sub = "ab build";
        assert!(s("build", &long_prefix) > s("build", short_sub));
        let long_word = format!("{} build", "y ".repeat(100));
        assert!(s("build", &long_word) > s("build", "rebuild"));
    }

    #[test]
    fn case_insensitive_and_unicode() {
        assert_eq!(s("BUILD", "build"), s("build", "BUILD"));
        assert_eq!(s("ÉCOLE", "école"), Some(TIER_EXACT));
        assert!(s("日本", "日本語のメモ").is_some());
        assert!(s("語", "日本語のメモ").is_some());
        assert!(s("😀", "hello 😀 world").is_some());
        assert!(s("ß", "İstanbul").is_none());
    }

    #[test]
    fn odd_input_does_not_panic() {
        for q in [
            "",
            " ",
            "\u{0}",
            "\n\t",
            "a\u{301}",
            "😀😀😀",
            "%_\\'\"",
            "İ",
        ] {
            for t in [
                "",
                " ",
                "İİİ",
                "a\u{301}b",
                "\u{0}",
                "😀",
                "long ".repeat(500).as_str(),
            ] {
                let _ = score(q, t);
            }
        }
        let huge = "ab".repeat(10_000);
        let _ = score(&huge, &huge);
        let _ = score("a b c d e f g", "g f e d c b a");
    }

    #[test]
    fn blank_query_matches_everything_with_zero() {
        assert_eq!(s("", "anything"), Some(0));
        assert_eq!(s("   ", ""), Some(0));
    }

    #[test]
    fn no_match_is_none() {
        assert!(s("xyz", "build").is_none());
        assert!(s("bldz", "build").is_none());
        assert!(s("abc", "").is_none());
    }

    #[test]
    fn word_prefix_requires_boundary() {
        let word = s("dep", "Build deploy").unwrap();
        let inner = s("dep", "Rebuild redeploy").unwrap();
        assert!(word > inner);
        assert!(s("dep", "my-deploy").unwrap() > inner);
    }

    #[test]
    fn multi_word_any_order_below_phrase() {
        let phrase = s("login bug", "Fix login bug").unwrap();
        let scattered = s("bug login", "Fix login bug").unwrap();
        assert!(phrase > scattered);
        assert!(s("bug login", "Fix login").is_none());
        assert!(s("bug login", "Bug in login form").is_some());
    }

    #[test]
    fn tighter_fuzzy_ranks_higher() {
        assert!(s("ntm", "new terminal main").unwrap() > s("ntm", "n t x x x x m").unwrap());
    }

    #[test]
    fn rank_is_stable_and_limited() {
        let items = ["beta", "alpha", "alphabet", "alpha"];
        // exact "alpha" at 1 and 3 (stable), then prefix "alphabet".
        assert_eq!(rank("alpha", &items, |s| s, 10), vec![1, 3, 2]);
        assert_eq!(rank("alpha", &items, |s| s, 2), vec![1, 3]);
        assert_eq!(rank("", &items, |s| s, 3), vec![0, 1, 2]);
        assert!(rank("zzz", &items, |s| s, 10).is_empty());
        assert!(rank("alpha", &items, |s| s, 0).is_empty());
    }

    fn item(group: Group, title: &str, target: Target) -> Item {
        Item {
            group,
            title: title.to_string(),
            detail: String::new(),
            target,
        }
    }

    fn sample() -> Sources {
        let p = ProjectId(1);
        Sources {
            commands: Sources::command_items(),
            projects: vec![
                item(Group::Projects, "Terminal Project", Target::Project(p)),
                item(Group::Projects, "Other", Target::Project(ProjectId(2))),
            ],
            tasks: vec![
                item(
                    Group::Tasks,
                    "Fix terminal crash",
                    Target::Task {
                        project: p,
                        task: TaskId(1),
                    },
                ),
                item(
                    Group::Tasks,
                    "Unrelated",
                    Target::Task {
                        project: p,
                        task: TaskId(2),
                    },
                ),
            ],
            notes: vec![item(
                Group::Notes,
                "terminal notes",
                Target::Note {
                    project: p,
                    note: NoteId(1),
                    task: None,
                    title: "terminal notes".into(),
                },
            )],
            note_text: vec![item(
                Group::NoteText,
                "meeting notes",
                Target::Note {
                    project: p,
                    note: NoteId(2),
                    task: None,
                    title: "meeting notes".into(),
                },
            )],
            commits: vec![item(Group::Commits, "unrelated title", Target::Commit)],
        }
    }

    #[test]
    fn blank_query_shows_commands_and_recent_but_no_commits() {
        let out = assemble("  ", sample());
        assert_eq!(
            out.iter().filter(|i| i.group == Group::Commands).count(),
            Command::ALL.len()
        );
        assert!(out.iter().any(|i| i.group == Group::Projects));
        assert!(out.iter().any(|i| i.group == Group::Tasks));
        assert!(!out.iter().any(|i| i.group == Group::Commits));
    }

    #[test]
    fn query_groups_in_display_order_and_filters() {
        let out = assemble("terminal", sample());
        let groups: Vec<Group> = out.iter().map(|i| i.group).collect();
        let mut sorted = groups.clone();
        sorted.sort();
        assert_eq!(groups, sorted, "groups must be contiguous and ordered");
        assert!(out
            .iter()
            .all(|i| i.title != "Unrelated" && i.title != "Other"));
        // Commits are trusted to be pre-filtered by git and kept.
        assert!(out.iter().any(|i| i.group == Group::Commits));
        // Both terminal commands match, exact-ish ones first.
        assert_eq!(out[0].target, Target::Command(Command::NewTerminal));
    }

    #[test]
    fn note_text_hits_skip_title_ranking_and_sit_between_notes_and_commits() {
        // "meeting notes" does not contain "terminal", yet its text matched.
        let out = assemble("terminal", sample());
        let groups: Vec<Group> = out.iter().map(|i| i.group).collect();
        let text_at = groups.iter().position(|g| *g == Group::NoteText).unwrap();
        let notes_at = groups.iter().position(|g| *g == Group::Notes).unwrap();
        let commits_at = groups.iter().position(|g| *g == Group::Commits).unwrap();
        assert!(notes_at < text_at && text_at < commits_at);
        assert_eq!(Group::NoteText.label(), "IN NOTES");
        // A blank query shows recents only, never text hits.
        assert!(!assemble("  ", sample())
            .iter()
            .any(|i| i.group == Group::NoteText));
    }

    #[test]
    fn every_command_has_a_unique_title_and_shortcut_hint() {
        let mut titles = std::collections::HashSet::new();
        let mut hints = std::collections::HashSet::new();
        for command in Command::ALL {
            assert!(titles.insert(command.title()), "duplicate title");
            // Commands without a shortcut have an empty hint; real ones
            // must not share a key.
            if !command.hint().is_empty() {
                assert!(hints.insert(command.hint()), "duplicate shortcut hint");
            }
        }
        // All of them fit in the commands group on a blank query.
        assert!(Command::ALL.len() <= Group::Commands.cap(true));
    }

    #[test]
    fn total_is_capped() {
        let many = |g: Group| -> Vec<Item> {
            (0..200)
                .map(|n| item(g, &format!("item {n}"), Target::Commit))
                .collect()
        };
        let src = Sources {
            commands: Vec::new(),
            projects: many(Group::Projects),
            tasks: many(Group::Tasks),
            notes: many(Group::Notes),
            note_text: Vec::new(),
            commits: many(Group::Commits),
        };
        let out = assemble("item", src);
        assert!(out.len() <= MAX_RESULTS);
        assert_eq!(out.iter().filter(|i| i.group == Group::Tasks).count(), 12);
    }

    #[test]
    fn highlight_wraps() {
        assert_eq!(step_highlight(0, -1, 3), 2);
        assert_eq!(step_highlight(2, 1, 3), 0);
        assert_eq!(step_highlight(1, 1, 3), 2);
        assert_eq!(step_highlight(5, 1, 0), 0);
    }
}
