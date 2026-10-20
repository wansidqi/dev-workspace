//! Pure logic behind the Changes view (no GPUI): flattening repos and files
//! into one list, choosing which repos a commit goes to, and turning clicks
//! on diff lines into a [`LineSelection`]. Kept out of the view so it can be
//! unit tested (CLAUDE.md: keep logic out of views).

use devspace_core::RepoId;
use devspace_git::{DiffSide, FileDiff, LineKind, LineSelection, RepoStatus};

/// One row of the left-hand list, which interleaves every repo's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Header of the repo at this index in the view's repo list.
    Repo(usize),
    /// File `file` of repo `repo`.
    File { repo: usize, file: usize },
    /// A repo with nothing to commit (or one that couldn't be read).
    Note(usize),
}

/// What the list needs to know about one repo to lay itself out.
pub struct RepoRows<'a> {
    pub id: RepoId,
    pub status: Option<&'a RepoStatus>,
    pub collapsed: bool,
    /// Reading the repo failed; shown as a note under the header.
    pub failed: bool,
}

/// Rows for every repo that passes `filter` (all of them when `None`):
/// header, then its files unless collapsed, or a single note row when there
/// is nothing to list.
pub fn rows(repos: &[RepoRows<'_>], filter: Option<RepoId>) -> Vec<Row> {
    let mut out = Vec::new();
    for (ri, repo) in repos.iter().enumerate() {
        if filter.is_some_and(|f| f != repo.id) {
            continue;
        }
        out.push(Row::Repo(ri));
        if repo.collapsed {
            continue;
        }
        let files = repo.status.map(|s| s.files.len()).unwrap_or(0);
        if files == 0 || repo.failed {
            out.push(Row::Note(ri));
        } else {
            out.extend((0..files).map(|file| Row::File { repo: ri, file }));
        }
    }
    out
}

/// Whether a header checkbox should stage everything or unstage everything:
/// stage while anything is left unstaged, otherwise unstage.
pub fn header_toggle_stages(status: &RepoStatus) -> bool {
    status.files.iter().any(|f| !f.is_fully_staged())
}

/// Repos a commit goes to: the single filtered repo, or every repo that has
/// something staged. Amending always needs exactly one.
pub fn commit_targets(repos: &[(RepoId, &RepoStatus)], filter: Option<RepoId>) -> Vec<RepoId> {
    repos
        .iter()
        .filter(|(id, status)| match filter {
            Some(f) => *id == f && status.staged_count() > 0,
            None => status.staged_count() > 0,
        })
        .map(|(id, _)| *id)
        .collect()
}

/// A row of the diff viewer. The diff is flattened so it can be virtualized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    Hunk(usize),
    Line { hunk: usize, line: usize },
}

pub fn diff_rows(diff: &FileDiff) -> Vec<DiffRow> {
    let mut out = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        out.push(DiffRow::Hunk(h));
        for (l, line) in hunk.lines.iter().enumerate() {
            // The "no newline" marker is shown as a faint note, not a row of
            // its own the user could select.
            if line.kind != LineKind::NoNewline {
                out.push(DiffRow::Line { hunk: h, line: l });
            }
        }
    }
    out
}

/// Changed (selectable) lines in display order.
pub fn changed_lines(diff: &FileDiff) -> Vec<(usize, usize)> {
    diff_rows(diff)
        .into_iter()
        .filter_map(|row| match row {
            DiffRow::Line { hunk, line }
                if matches!(
                    diff.hunks[hunk].lines[line].kind,
                    LineKind::Added | LineKind::Removed
                ) =>
            {
                Some((hunk, line))
            }
            _ => None,
        })
        .collect()
}

/// Applies a click on a changed line: a plain click toggles it, a Shift-click
/// selects (or clears, if the clicked line was selected) everything between
/// the anchor and it. Returns the new anchor.
pub fn click_line(
    diff: &FileDiff,
    selection: &mut LineSelection,
    anchor: Option<(usize, usize)>,
    clicked: (usize, usize),
    shift: bool,
) -> Option<(usize, usize)> {
    let order = changed_lines(diff);
    if shift {
        let range = anchor
            .and_then(|a| order.iter().position(|l| *l == a))
            .zip(order.iter().position(|l| *l == clicked))
            .map(|(a, b)| (a.min(b), a.max(b)));
        if let Some((from, to)) = range {
            let select = !selection.contains(&clicked);
            for line in &order[from..=to] {
                if select {
                    selection.insert(*line);
                } else {
                    selection.remove(line);
                }
            }
            return Some(clicked);
        }
    }
    if !selection.remove(&clicked) {
        selection.insert(clicked);
    }
    Some(clicked)
}

/// Drops selected lines that no longer exist in `diff` (after a refresh).
pub fn retain_valid(diff: &FileDiff, selection: &mut LineSelection) {
    let valid: std::collections::HashSet<_> = changed_lines(diff).into_iter().collect();
    selection.retain(|l| valid.contains(l));
}

/// Which side to show for a file: keep the current one if it has changes,
/// otherwise whichever side does (unstaged first).
pub fn pick_side(current: DiffSide, has_unstaged: bool, has_staged: bool) -> DiffSide {
    match (current, has_unstaged, has_staged) {
        (DiffSide::Staged, _, true) => DiffSide::Staged,
        (DiffSide::Unstaged, true, _) => DiffSide::Unstaged,
        (_, true, _) => DiffSide::Unstaged,
        (_, false, true) => DiffSide::Staged,
        _ => current,
    }
}

/// "api, fe" / "api, fe and web" for a notice naming repos.
pub fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devspace_git::{ChangeKind, ChangedFile, DiffHunk, DiffLine};

    fn file(path: &str, staged: Option<ChangeKind>, unstaged: Option<ChangeKind>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            staged,
            unstaged,
        }
    }

    fn status(files: Vec<ChangedFile>) -> RepoStatus {
        RepoStatus {
            files,
            ..RepoStatus::default()
        }
    }

    #[test]
    fn rows_group_files_under_repo_headers() {
        let api = status(vec![]);
        let fe = status(vec![
            file("a.ts", None, Some(ChangeKind::Modified)),
            file("b.ts", Some(ChangeKind::Added), None),
        ]);
        let repos = [
            RepoRows {
                id: RepoId(1),
                status: Some(&api),
                collapsed: false,
                failed: false,
            },
            RepoRows {
                id: RepoId(2),
                status: Some(&fe),
                collapsed: false,
                failed: false,
            },
        ];
        assert_eq!(
            rows(&repos, None),
            [
                Row::Repo(0),
                Row::Note(0),
                Row::Repo(1),
                Row::File { repo: 1, file: 0 },
                Row::File { repo: 1, file: 1 },
            ]
        );
        assert_eq!(
            rows(&repos, Some(RepoId(2))),
            [
                Row::Repo(1),
                Row::File { repo: 1, file: 0 },
                Row::File { repo: 1, file: 1 },
            ]
        );
    }

    #[test]
    fn collapsed_repos_hide_their_files_and_unread_repos_show_a_note() {
        let fe = status(vec![file("a.ts", None, Some(ChangeKind::Modified))]);
        let repos = [
            RepoRows {
                id: RepoId(1),
                status: Some(&fe),
                collapsed: true,
                failed: false,
            },
            RepoRows {
                id: RepoId(2),
                status: None,
                collapsed: false,
                failed: true,
            },
        ];
        assert_eq!(
            rows(&repos, None),
            [Row::Repo(0), Row::Repo(1), Row::Note(1)]
        );
    }

    #[test]
    fn header_checkbox_stages_until_everything_is_staged() {
        let mixed = status(vec![
            file("a", Some(ChangeKind::Modified), None),
            file("b", None, Some(ChangeKind::Modified)),
        ]);
        assert!(header_toggle_stages(&mixed));
        let partial = status(vec![file(
            "a",
            Some(ChangeKind::Modified),
            Some(ChangeKind::Modified),
        )]);
        assert!(header_toggle_stages(&partial));
        let all = status(vec![file("a", Some(ChangeKind::Modified), None)]);
        assert!(!header_toggle_stages(&all));
    }

    #[test]
    fn commit_goes_to_repos_with_staged_files() {
        let a = status(vec![file("a", Some(ChangeKind::Added), None)]);
        let b = status(vec![file("b", None, Some(ChangeKind::Modified))]);
        let c = status(vec![file("c", Some(ChangeKind::Modified), None)]);
        let repos = [(RepoId(1), &a), (RepoId(2), &b), (RepoId(3), &c)];
        assert_eq!(commit_targets(&repos, None), [RepoId(1), RepoId(3)]);
        assert_eq!(commit_targets(&repos, Some(RepoId(3))), [RepoId(3)]);
        assert!(commit_targets(&repos, Some(RepoId(2))).is_empty());
    }

    fn sample_diff() -> FileDiff {
        let line = |kind, text: &str| DiffLine {
            kind,
            old_lineno: None,
            new_lineno: None,
            text: text.into(),
        };
        FileDiff {
            path: "a.txt".into(),
            hunks: vec![
                DiffHunk {
                    header: "@@ -1,3 +1,3 @@".into(),
                    old_start: 1,
                    old_lines: 3,
                    new_start: 1,
                    new_lines: 3,
                    lines: vec![
                        line(LineKind::Context, "a"),
                        line(LineKind::Removed, "b"),
                        line(LineKind::Added, "B"),
                        line(LineKind::NoNewline, ""),
                        line(LineKind::Added, "c"),
                    ],
                },
                DiffHunk {
                    header: "@@ -9 +9 @@".into(),
                    old_start: 9,
                    old_lines: 1,
                    new_start: 9,
                    new_lines: 1,
                    lines: vec![line(LineKind::Removed, "x"), line(LineKind::Added, "y")],
                },
            ],
            ..FileDiff::default()
        }
    }

    #[test]
    fn diff_rows_skip_the_no_newline_marker() {
        let rows = diff_rows(&sample_diff());
        assert_eq!(rows.len(), 8, "2 hunk headers + 6 lines");
        assert!(!rows.contains(&DiffRow::Line { hunk: 0, line: 3 }));
        assert_eq!(
            changed_lines(&sample_diff()),
            [(0, 1), (0, 2), (0, 4), (1, 0), (1, 1)]
        );
    }

    #[test]
    fn plain_click_toggles_and_shift_click_extends_across_hunks() {
        let d = sample_diff();
        let mut sel = LineSelection::new();
        let anchor = click_line(&d, &mut sel, None, (0, 1), false);
        assert_eq!(sel, LineSelection::from([(0, 1)]));
        let anchor = click_line(&d, &mut sel, anchor, (1, 0), true);
        assert_eq!(anchor, Some((1, 0)));
        assert_eq!(
            sel,
            LineSelection::from([(0, 1), (0, 2), (0, 4), (1, 0)]),
            "range runs through the hunk boundary and skips non-changes"
        );
        // Shift-click on a selected line clears the range back to the anchor.
        click_line(&d, &mut sel, Some((0, 1)), (0, 4), true);
        assert_eq!(sel, LineSelection::from([(1, 0)]));
        // Plain click toggles off.
        click_line(&d, &mut sel, None, (1, 0), false);
        assert!(sel.is_empty());
    }

    #[test]
    fn stale_selection_is_trimmed_after_a_refresh() {
        let d = sample_diff();
        let mut sel = LineSelection::from([(0, 1), (5, 5), (0, 0)]);
        retain_valid(&d, &mut sel);
        assert_eq!(sel, LineSelection::from([(0, 1)]));
    }

    #[test]
    fn side_follows_where_the_changes_are() {
        use DiffSide::*;
        assert_eq!(pick_side(Unstaged, true, true), Unstaged);
        assert_eq!(pick_side(Staged, true, true), Staged);
        assert_eq!(pick_side(Unstaged, false, true), Staged);
        assert_eq!(pick_side(Staged, true, false), Unstaged);
    }

    #[test]
    fn names_join_naturally() {
        assert_eq!(join_names(&[]), "");
        assert_eq!(join_names(&["api".into()]), "api");
        assert_eq!(join_names(&["api".into(), "fe".into()]), "api and fe");
        assert_eq!(
            join_names(&["api".into(), "fe".into(), "web".into()]),
            "api, fe and web"
        );
    }
}
