//! Monthly timesheet logic: turn git commits into task rows (one per ticket
//! number), suggest hours, keep hand edits across re-syncs, and lay the
//! report out as tab-separated text for pasting into a spreadsheet.
//!
//! Pure functions only; reading git and saving rows live in other crates.
//! Dates are `YYYY-MM-DD` strings and months `YYYY-MM`, which sort correctly
//! as text and need no date library (CLAUDE.md: keep dependencies minimal).

use std::collections::{HashMap, HashSet};

/// Constants of the hours estimate. Defaults match the values the old
/// standalone "git sheet" tool used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Hours every task starts with.
    pub base: f64,
    /// Hours added per square root of changed lines (big changes cost more,
    /// with diminishing returns).
    pub line_factor: f64,
    /// Rework: hours added per commit after the first.
    pub per_extra_commit: f64,
    /// Hours added per extra active day.
    pub per_extra_day: f64,
    /// Cap per active day.
    pub max_per_day: f64,
}

impl Default for Estimate {
    fn default() -> Self {
        Self {
            base: 0.5,
            line_factor: 0.15,
            per_extra_commit: 0.25,
            per_extra_day: 0.5,
            max_per_day: 6.0,
        }
    }
}

/// Per-user timesheet settings (one set for the whole workspace).
#[derive(Debug, Clone, PartialEq)]
pub struct TimesheetSettings {
    pub name: String,
    pub role: String,
    /// Sheet title, e.g. "Project Monthly Report".
    pub title: String,
    /// Comma-separated git author names or emails; empty means everyone.
    pub authors: String,
    pub estimate: Estimate,
    /// A project's table is padded with blank rows up to this many.
    pub min_rows: usize,
}

impl Default for TimesheetSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            role: "FE Developer".to_string(),
            title: "Monthly Report".to_string(),
            authors: String::new(),
            estimate: Estimate::default(),
            min_rows: 8,
        }
    }
}

impl TimesheetSettings {
    /// The author filters as a clean list (trimmed, blanks dropped).
    pub fn author_list(&self) -> Vec<String> {
        self.authors
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// What the timesheet keeps of a commit. A snapshot, not a link: a report
/// must still read the same after a rebase or after the repo is unlinked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimesheetCommit {
    /// Display name of the repo the commit came from.
    pub repo: String,
    pub hash: String,
    pub subject: String,
    /// Author date, `YYYY-MM-DD`.
    pub date: String,
    /// Changed lines, excluding lockfiles and generated output.
    pub lines: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSource {
    /// Built from commits.
    Git,
    /// Typed in by hand; never touched by a git sync.
    Manual,
}

impl RowSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RowSource::Git => "git",
            RowSource::Manual => "manual",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "git" => Some(RowSource::Git),
            "manual" => Some(RowSource::Manual),
            _ => None,
        }
    }
}

/// One task line of a project's monthly table.
#[derive(Debug, Clone, PartialEq)]
pub struct TimesheetRow {
    /// Ticket number, short commit hash, or `manual:<n>`. Identifies the row
    /// across re-syncs within a project and month.
    pub key: String,
    pub title: String,
    pub date_from: String,
    pub date_to: String,
    pub hours: Option<f64>,
    /// Hours were estimated from git and may be recalculated; cleared once
    /// the user types a value.
    pub auto: bool,
    pub source: RowSource,
    pub commits: Vec<TimesheetCommit>,
}

impl TimesheetRow {
    /// A hand-typed row dated `date`. `key` must be unique in the table.
    pub fn manual(key: String, date: &str) -> Self {
        Self {
            key,
            title: String::new(),
            date_from: date.to_string(),
            date_to: date.to_string(),
            hours: None,
            auto: false,
            source: RowSource::Manual,
            commits: Vec::new(),
        }
    }

    pub fn day_count(&self) -> usize {
        self.commits
            .iter()
            .map(|c| c.date.as_str())
            .collect::<HashSet<_>>()
            .len()
    }

    pub fn line_count(&self) -> u32 {
        self.commits.iter().map(|c| c.lines).sum()
    }
}

/// Suggested hours for a row's commits, rounded to half hours, at least 0.5.
/// `None` when there are no commits to base it on.
pub fn suggest_hours(commits: &[TimesheetCommit], e: &Estimate) -> Option<f64> {
    if commits.is_empty() {
        return None;
    }
    let days = commits
        .iter()
        .map(|c| c.date.as_str())
        .collect::<HashSet<_>>()
        .len() as f64;
    let lines: f64 = commits.iter().map(|c| f64::from(c.lines)).sum();
    let n = commits.len() as f64;
    let raw = e.base
        + e.line_factor * lines.sqrt()
        + e.per_extra_commit * (n - 1.0)
        + e.per_extra_day * (days - 1.0);
    let capped = raw.min(e.max_per_day * days);
    Some(((capped * 2.0).round() / 2.0).max(0.5))
}

/// Splits `7267: fix x`, `#7267 - fix x` or `[7267] fix x` into the ticket
/// number (three or more digits) and the remaining text. Subjects without a
/// ticket come back whole.
pub fn parse_subject(subject: &str) -> (Option<String>, String) {
    let s = subject.trim_start();
    let s = s
        .strip_prefix('#')
        .or_else(|| s.strip_prefix('['))
        .unwrap_or(s);
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits < 3 {
        return (None, subject.trim().to_string());
    }
    let ticket = s[..digits].to_string();
    let rest = s[digits..].strip_prefix(']').unwrap_or(&s[digits..]);
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix([':', '-', '\u{2013}'])
        .unwrap_or(rest)
        .trim();
    let text = if rest.is_empty() {
        subject.trim()
    } else {
        rest
    };
    (Some(ticket), text.to_string())
}

/// Groups commits into task rows: one per ticket number, or one per commit
/// when the subject has none. A commit is identified by (repo, hash), so the
/// same commit seen twice is counted once while equal hashes in different
/// repos stay separate. Rows come out in first-commit order, commits oldest
/// first. Hours are left unset; see [`merge_rows`].
pub fn group_commits(mut commits: Vec<TimesheetCommit>) -> Vec<TimesheetRow> {
    commits.sort_by(|a, b| a.date.cmp(&b.date));
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut rows: Vec<TimesheetRow> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for c in commits {
        if !seen.insert((c.repo.clone(), c.hash.clone())) {
            continue;
        }
        let (ticket, text) = parse_subject(&c.subject);
        let key = ticket
            .clone()
            .unwrap_or_else(|| c.hash.chars().take(8).collect());
        if let Some(&i) = index.get(&key) {
            let row = &mut rows[i];
            row.date_to = c.date.clone();
            row.commits.push(c);
        } else {
            index.insert(key.clone(), rows.len());
            rows.push(TimesheetRow {
                key,
                title: match &ticket {
                    Some(t) => format!("{t} - {text}"),
                    None => text,
                },
                date_from: c.date.clone(),
                date_to: c.date.clone(),
                hours: None,
                auto: true,
                source: RowSource::Git,
                commits: vec![c],
            });
        }
    }
    rows
}

/// Rebuilds a table from fresh git rows while keeping the user's work:
/// typed hours and edited titles survive, estimated hours are recalculated
/// from the fresh commits, and manual rows are carried over at the end.
pub fn merge_rows(
    fresh: Vec<TimesheetRow>,
    previous: &[TimesheetRow],
    estimate: &Estimate,
) -> Vec<TimesheetRow> {
    let old: HashMap<&str, &TimesheetRow> = previous.iter().map(|r| (r.key.as_str(), r)).collect();
    let mut out: Vec<TimesheetRow> = fresh
        .into_iter()
        .map(|mut row| {
            let prev = old
                .get(row.key.as_str())
                .filter(|p| p.source == RowSource::Git);
            if let Some(p) = prev {
                row.title = p.title.clone();
            }
            match prev {
                Some(p) if !p.auto && p.hours.is_some() => {
                    row.hours = p.hours;
                    row.auto = false;
                }
                _ => {
                    row.hours = suggest_hours(&row.commits, estimate);
                    row.auto = true;
                }
            }
            row
        })
        .collect();
    out.extend(
        previous
            .iter()
            .filter(|r| r.source == RowSource::Manual)
            .cloned(),
    );
    out
}

// ---- dates and months ----

/// Days since the Unix epoch to a (year, month, day) civil date (Howard
/// Hinnant's `civil_from_days`, public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD` for a unix time in seconds. Add the author's UTC offset
/// first to get their local calendar day.
pub fn date_from_unix(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM` of a unix time (UTC).
pub fn month_from_unix(secs: i64) -> String {
    let mut s = date_from_unix(secs);
    s.truncate(7);
    s
}

/// Splits `YYYY-MM`; `None` unless it names a real month.
pub fn parse_month(month: &str) -> Option<(i32, u32)> {
    let (y, m) = month.split_once('-')?;
    let year: i32 = y.parse().ok()?;
    let month: u32 = m.parse().ok()?;
    ((1..=9999).contains(&year) && (1..=12).contains(&month)).then_some((year, month))
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// First and last day of a month, inclusive, as `YYYY-MM-DD`.
pub fn month_bounds(month: &str) -> Option<(String, String)> {
    let (y, m) = parse_month(month)?;
    Some((
        format!("{y:04}-{m:02}-01"),
        format!("{y:04}-{m:02}-{:02}", days_in_month(y, m)),
    ))
}

/// The month `delta` months away; `None` if `month` is invalid or the result
/// leaves years 1 to 9999.
pub fn shift_month(month: &str, delta: i32) -> Option<String> {
    let (y, m) = parse_month(month)?;
    let index = y * 12 + (m as i32 - 1) + delta;
    let (year, m0) = (index.div_euclid(12), index.rem_euclid(12));
    (1..=9999)
        .contains(&year)
        .then(|| format!("{year:04}-{:02}", m0 + 1))
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// "October 2026" for `2026-10`; the input unchanged if it is not a month.
pub fn month_label(month: &str) -> String {
    match parse_month(month) {
        Some((y, m)) => format!("{} {y}", MONTH_NAMES[m as usize - 1]),
        None => month.to_string(),
    }
}

fn split_date(iso: &str) -> Option<(u32, u32, u32)> {
    let mut it = iso.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    it.next().is_none().then_some((y, m, d))
}

/// `2026-10-05` as `5/10/2026`; unparseable input comes back unchanged.
pub fn fmt_date(iso: &str) -> String {
    match split_date(iso) {
        Some((y, m, d)) => format!("{d}/{m}/{y}"),
        None => iso.to_string(),
    }
}

/// One date, or `from - to` when the task spans days.
pub fn fmt_range(from: &str, to: &str) -> String {
    if from == to {
        fmt_date(from)
    } else {
        format!("{} - {}", fmt_date(from), fmt_date(to))
    }
}

/// Hours as the sheet shows them: `6`, `6.5`.
pub fn fmt_hours(hours: f64) -> String {
    format!("{hours}")
}

/// Sum of the hours a table has entered.
pub fn total_hours(rows: &[TimesheetRow]) -> f64 {
    rows.iter().filter_map(|r| r.hours).sum()
}

// ---- clipboard text ----

fn tsv_cell(text: &str) -> String {
    if text.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// The report as tab-separated text in the monthly sheet layout, ready to
/// paste at cell A1. Each project's first data row carries a `=SUM(...)`
/// formula over its hours column. `projects` is (heading, rows) in sheet
/// order; every table is padded with blank rows up to `settings.min_rows`.
pub fn clipboard_text(
    settings: &TimesheetSettings,
    month: &str,
    remarks: &str,
    projects: &[(String, Vec<TimesheetRow>)],
) -> String {
    let mut lines: Vec<Vec<String>> = Vec::new();
    let blank = |lines: &mut Vec<Vec<String>>, n: usize| {
        lines.extend(std::iter::repeat_with(|| vec![String::new()]).take(n));
    };

    lines.push(vec![settings.title.clone()]);
    blank(&mut lines, 1);
    for (label, value) in [
        ("Resources Name:", settings.name.clone()),
        ("Resources Role:", settings.role.clone()),
        ("Period:", month_label(month)),
        ("REMARKS:", remarks.to_string()),
    ] {
        lines.push(vec![label.to_string(), value]);
    }
    blank(&mut lines, 2);

    for (name, rows) in projects {
        let n = rows.len().max(settings.min_rows);
        // 1-based sheet row of the column header; data starts below it.
        let header_row = lines.len() + 2;
        let (first, last) = (header_row + 1, header_row + n);
        lines.push(vec![name.clone()]);
        lines.push(
            ["No.", "Tasks", "Date", "Time Spent (Hours)", "Total"]
                .map(str::to_string)
                .to_vec(),
        );
        for i in 0..n {
            let row = rows.get(i);
            lines.push(vec![
                (i + 1).to_string(),
                row.map(|r| r.title.clone()).unwrap_or_default(),
                row.map(|r| fmt_range(&r.date_from, &r.date_to))
                    .unwrap_or_default(),
                row.and_then(|r| r.hours.map(fmt_hours)).unwrap_or_default(),
                if i == 0 {
                    format!("=SUM(D{first}:D{last})")
                } else {
                    String::new()
                },
            ]);
        }
        blank(&mut lines, 3);
    }

    lines
        .iter()
        .map(|cells| {
            cells
                .iter()
                .map(|c| tsv_cell(c))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(repo: &str, hash: &str, subject: &str, date: &str, lines: u32) -> TimesheetCommit {
        TimesheetCommit {
            repo: repo.into(),
            hash: hash.into(),
            subject: subject.into(),
            date: date.into(),
            lines,
        }
    }

    #[test]
    fn parses_ticket_subjects() {
        let ticket = |s: &str| parse_subject(s);
        assert_eq!(ticket("7267: fix x"), (Some("7267".into()), "fix x".into()));
        assert_eq!(
            ticket("#7267 - fix x"),
            (Some("7267".into()), "fix x".into())
        );
        assert_eq!(
            ticket("[7267] fix x"),
            (Some("7267".into()), "fix x".into())
        );
        assert_eq!(
            ticket("7267 \u{2013} fix x"),
            (Some("7267".into()), "fix x".into())
        );
        assert_eq!(ticket("fix x"), (None, "fix x".into()));
        assert_eq!(ticket("12: too short"), (None, "12: too short".into()));
        // A bare ticket keeps the whole subject as its text.
        assert_eq!(ticket("7267"), (Some("7267".into()), "7267".into()));
    }

    #[test]
    fn groups_by_ticket_and_counts_each_commit_once() {
        let rows = group_commits(vec![
            commit("a", "bbbbbbbb1", "7267: second", "2026-10-03", 10),
            commit("a", "aaaaaaaa1", "7267: first", "2026-10-01", 4),
            commit("a", "aaaaaaaa1", "7267: first", "2026-10-01", 4),
            commit("a", "cccccccc1", "no ticket", "2026-10-02", 1),
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "7267");
        assert_eq!(rows[0].title, "7267 - first");
        assert_eq!(rows[0].commits.len(), 2);
        assert_eq!(
            (rows[0].date_from.as_str(), rows[0].date_to.as_str()),
            ("2026-10-01", "2026-10-03")
        );
        assert_eq!(rows[1].key, "cccccccc");
        assert_eq!(rows[1].title, "no ticket");
    }

    #[test]
    fn same_hash_in_two_repos_is_two_commits() {
        let rows = group_commits(vec![
            commit("a", "abcdef123", "no ticket", "2026-10-01", 1),
            commit("b", "abcdef123", "no ticket", "2026-10-01", 1),
        ]);
        assert_eq!(rows.len(), 1, "same key groups them");
        assert_eq!(rows[0].commits.len(), 2);
    }

    #[test]
    fn suggests_half_hours_with_a_floor_and_a_cap() {
        let e = Estimate::default();
        assert_eq!(suggest_hours(&[], &e), None);
        let one = [commit("a", "1", "x", "2026-10-01", 0)];
        assert_eq!(suggest_hours(&one, &e), Some(0.5));
        // 0.5 + 0.15*10 = 2.0
        let big = [commit("a", "1", "x", "2026-10-01", 100)];
        assert_eq!(suggest_hours(&big, &e), Some(2.0));
        // The per-day cap bites on a huge change.
        let huge = [commit("a", "1", "x", "2026-10-01", 1_000_000)];
        assert_eq!(suggest_hours(&huge, &e), Some(6.0));
    }

    #[test]
    fn merge_keeps_typed_hours_and_manual_rows_but_refreshes_estimates() {
        let e = Estimate::default();
        let commits = |lines| vec![commit("a", "h1", "7267: x", "2026-10-01", lines)];
        let mut typed = group_commits(commits(0)).remove(0);
        typed.hours = Some(3.0);
        typed.auto = false;
        typed.title = "My title".into();
        let manual = TimesheetRow::manual("manual:1".into(), "2026-10-02");

        let merged = merge_rows(
            group_commits(commits(400)),
            &[typed.clone(), manual.clone()],
            &e,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].hours, Some(3.0));
        assert!(!merged[0].auto);
        assert_eq!(merged[0].title, "My title");
        assert_eq!(merged[1], manual);

        // An estimated row is recalculated from the fresh commits.
        let mut auto = typed;
        auto.hours = Some(0.5);
        auto.auto = true;
        let merged = merge_rows(group_commits(commits(400)), &[auto], &e);
        assert_eq!(merged[0].hours, Some(3.5));
        assert!(merged[0].auto);
    }

    #[test]
    fn month_helpers() {
        assert_eq!(month_label("2026-10"), "October 2026");
        assert_eq!(month_label("junk"), "junk");
        assert_eq!(shift_month("2026-01", -1).as_deref(), Some("2025-12"));
        assert_eq!(shift_month("2026-12", 1).as_deref(), Some("2027-01"));
        assert_eq!(shift_month("2026-13", 1), None);
        assert_eq!(
            month_bounds("2024-02"),
            Some(("2024-02-01".into(), "2024-02-29".into()))
        );
        assert_eq!(
            month_bounds("2026-02"),
            Some(("2026-02-01".into(), "2026-02-28".into()))
        );
        assert_eq!(month_from_unix(0), "1970-01");
        assert_eq!(date_from_unix(1_792_281_600), "2026-10-18");
        assert_eq!(fmt_date("2026-10-05"), "5/10/2026");
        assert_eq!(fmt_range("2026-10-05", "2026-10-05"), "5/10/2026");
        assert_eq!(
            fmt_range("2026-10-05", "2026-10-07"),
            "5/10/2026 - 7/10/2026"
        );
        assert_eq!(fmt_hours(6.0), "6");
        assert_eq!(fmt_hours(6.5), "6.5");
    }

    #[test]
    fn clipboard_layout_pads_rows_and_sums_the_hours_column() {
        let settings = TimesheetSettings {
            name: "Ann".into(),
            min_rows: 3,
            ..TimesheetSettings::default()
        };
        let mut row = group_commits(vec![commit(
            "a",
            "h1",
            "7267: \"quoted\" fix",
            "2026-10-01",
            0,
        )])
        .remove(0);
        row.hours = Some(1.5);
        let text = clipboard_text(&settings, "2026-10", "none", &[("SRC".into(), vec![row])]);
        let lines: Vec<&str> = text.split('\n').collect();

        assert_eq!(lines[0], "Monthly Report");
        assert_eq!(lines[2], "Resources Name:\tAnn");
        assert_eq!(lines[4], "Period:\tOctober 2026");
        assert_eq!(lines[8], "SRC");
        assert_eq!(lines[9], "No.\tTasks\tDate\tTime Spent (Hours)\tTotal");
        // Header is sheet row 10, so data is rows 11 to 13.
        // The title contains quotes, so its cell is quoted with them doubled.
        assert_eq!(
            lines[10],
            "1\t\"7267 - \"\"quoted\"\" fix\"\t1/10/2026\t1.5\t=SUM(D11:D13)"
        );
        assert_eq!(lines[11], "2\t\t\t\t");
        assert_eq!(lines[12], "3\t\t\t\t");
    }
}
