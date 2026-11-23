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
    /// Header of the "Stashed changes" section of the repo at this index.
    StashHeader(usize),
    /// File `file` of that repo's stash.
    StashFile { repo: usize, file: usize },
    /// Banner under the header of a repo with an unfinished operation.
    Operation(usize),
}

/// What the list needs to know about one repo to lay itself out.
pub struct RepoRows<'a> {
    pub id: RepoId,
    pub status: Option<&'a RepoStatus>,
    pub collapsed: bool,
    /// Reading the repo failed; shown as a note under the header.
    pub failed: bool,
    /// Files in this repo's stash for the current branch (0 = no section).
    pub stash_files: usize,
    pub stash_collapsed: bool,
    /// A merge, rebase, cherry-pick or revert is unfinished (a banner row is shown).
    pub operation: bool,
}

/// Rows for every repo in `filter` (all of them when it is empty):
/// header, then its files unless collapsed, or a single note row when there
/// is nothing to list.
pub fn rows(repos: &[RepoRows<'_>], filter: &[RepoId]) -> Vec<Row> {
    let mut out = Vec::new();
    for (ri, repo) in repos.iter().enumerate() {
        if !filter.is_empty() && !filter.contains(&repo.id) {
            continue;
        }
        out.push(Row::Repo(ri));
        if repo.collapsed {
            continue;
        }
        if repo.operation && !repo.failed {
            out.push(Row::Operation(ri));
        }
        let files = repo.status.map(|s| s.files.len()).unwrap_or(0);
        if files == 0 || repo.failed {
            out.push(Row::Note(ri));
        } else {
            out.extend((0..files).map(|file| Row::File { repo: ri, file }));
        }
        if repo.stash_files > 0 && !repo.failed {
            out.push(Row::StashHeader(ri));
            if !repo.stash_collapsed {
                out.extend((0..repo.stash_files).map(|file| Row::StashFile { repo: ri, file }));
            }
        }
    }
    out
}

/// The file row `delta` places after `current` (`(repo, file)` indexes), skipping
/// headers and notes. Starts at the first file when nothing is selected, and
/// stays on the last or first file at the ends.
pub fn step_file(
    rows: &[Row],
    current: Option<(usize, usize)>,
    delta: i32,
) -> Option<(usize, usize)> {
    let files: Vec<(usize, usize)> = rows
        .iter()
        .filter_map(|r| match r {
            Row::File { repo, file } => Some((*repo, *file)),
            _ => None,
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    let Some(at) = current.and_then(|c| files.iter().position(|f| *f == c)) else {
        return Some(if delta >= 0 {
            files[0]
        } else {
            files[files.len() - 1]
        });
    };
    let next = (at as i64 + i64::from(delta)).clamp(0, files.len() as i64 - 1) as usize;
    Some(files[next])
}

/// Whether a header checkbox should stage everything or unstage everything:
/// stage while anything is left unstaged, otherwise unstage.
pub fn header_toggle_stages(status: &RepoStatus) -> bool {
    status.files.iter().any(|f| !f.is_fully_staged())
}

/// Repos a commit goes to: every filtered repo (every repo when the filter is
/// empty) that has something staged. Amending always needs exactly one.
pub fn commit_targets(repos: &[(RepoId, &RepoStatus)], filter: &[RepoId]) -> Vec<RepoId> {
    repos
        .iter()
        .filter(|(id, status)| {
            (filter.is_empty() || filter.contains(id)) && status.staged_count() > 0
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

/// What the history popover plans for one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanAction {
    Pick,
    Squash,
    Fixup,
    Drop,
    Reword(String),
}

impl PlanAction {
    pub fn label(&self) -> &'static str {
        match self {
            PlanAction::Pick => "Pick",
            PlanAction::Squash => "Squash",
            PlanAction::Fixup => "Fixup",
            PlanAction::Drop => "Drop",
            PlanAction::Reword(_) => "Reword",
        }
    }
}

/// One commit of the rewrite plan. The plan is listed newest first (like
/// history); "squash" folds a commit into the one *below* it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    pub hash: String,
    pub summary: String,
    pub unpushed: bool,
    pub action: PlanAction,
}

/// Cycles the action of row `ix` through pick, squash, fixup and drop. The
/// oldest row (last) has nothing below it to join, so it skips squash and
/// fixup. A reword goes back to pick.
pub fn cycle_action(plan: &mut [PlanItem], ix: usize) {
    let last = plan.len().saturating_sub(1);
    let Some(item) = plan.get_mut(ix) else { return };
    item.action = match (&item.action, ix == last) {
        (PlanAction::Pick | PlanAction::Reword(_), false) => PlanAction::Squash,
        (PlanAction::Pick | PlanAction::Reword(_), true) => PlanAction::Drop,
        (PlanAction::Squash, _) => PlanAction::Fixup,
        (PlanAction::Fixup, _) => PlanAction::Drop,
        (PlanAction::Drop, _) => PlanAction::Pick,
    };
}

/// Moves row `ix` one place up (`up`) or down. Returns the new index.
pub fn move_item(plan: &mut [PlanItem], ix: usize, up: bool) -> usize {
    let to = if up { ix.checked_sub(1) } else { Some(ix + 1) };
    match to {
        Some(to) if ix < plan.len() && to < plan.len() => {
            plan.swap(ix, to);
            to
        }
        _ => ix,
    }
}

/// Whether running the plan would change anything.
pub fn plan_changes_history(plan: &[PlanItem], original: &[String]) -> bool {
    plan.iter().any(|p| p.action != PlanAction::Pick)
        || plan
            .iter()
            .map(|p| p.hash.as_str())
            .ne(original.iter().map(String::as_str))
}

/// Whether the plan rewrites a commit that is already on the remote (so the
/// branch will need a force push afterwards).
pub fn plan_touches_pushed(plan: &[PlanItem], original: &[String]) -> bool {
    // Pushed commits are the oldest (last) rows. A rebase replays every row
    // from the oldest changed one up to the newest, so the push is rewritten
    // when the first pushed row is at or above the last changed row.
    // A squash or fixup also rewrites the commit below it, which it joins.
    let reach = |ix: usize| -> Option<usize> {
        let moved = original.get(ix) != Some(&plan[ix].hash);
        match plan[ix].action {
            PlanAction::Squash | PlanAction::Fixup => Some(ix + 1),
            PlanAction::Pick if !moved => None,
            _ => Some(ix),
        }
    };
    let Some(last_changed) = (0..plan.len()).filter_map(reach).max() else {
        return false;
    };
    plan.iter()
        .position(|p| !p.unpushed)
        .is_some_and(|first_pushed| first_pushed <= last_changed)
}

/// The plan as git wants it: oldest first.
pub fn plan_entries(plan: &[PlanItem]) -> Vec<devspace_git::RebaseEntry> {
    plan.iter()
        .rev()
        .map(|p| devspace_git::RebaseEntry {
            hash: p.hash.clone(),
            action: match &p.action {
                PlanAction::Pick => devspace_git::RebaseAction::Pick,
                PlanAction::Squash => devspace_git::RebaseAction::Squash,
                PlanAction::Fixup => devspace_git::RebaseAction::Fixup,
                PlanAction::Drop => devspace_git::RebaseAction::Drop,
                PlanAction::Reword(m) => devspace_git::RebaseAction::Reword(m.clone()),
            },
        })
        .collect()
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
                stash_files: 0,
                stash_collapsed: false,
                operation: false,
            },
            RepoRows {
                id: RepoId(2),
                status: Some(&fe),
                collapsed: false,
                failed: false,
                stash_files: 0,
                stash_collapsed: false,
                operation: false,
            },
        ];
        assert_eq!(
            rows(&repos, &[]),
            [
                Row::Repo(0),
                Row::Note(0),
                Row::Repo(1),
                Row::File { repo: 1, file: 0 },
                Row::File { repo: 1, file: 1 },
            ]
        );
        assert_eq!(
            rows(&repos, &[RepoId(2)]),
            [
                Row::Repo(1),
                Row::File { repo: 1, file: 0 },
                Row::File { repo: 1, file: 1 },
            ]
        );
    }

    #[test]
    fn the_stash_section_follows_the_files_and_collapses() {
        let fe = status(vec![file("a.ts", None, Some(ChangeKind::Modified))]);
        let clean = status(vec![]);
        let mut repos = [
            RepoRows {
                id: RepoId(1),
                status: Some(&fe),
                collapsed: false,
                failed: false,
                stash_files: 2,
                stash_collapsed: false,
                operation: false,
            },
            RepoRows {
                id: RepoId(2),
                status: Some(&clean),
                collapsed: false,
                failed: false,
                stash_files: 1,
                stash_collapsed: true,
                operation: false,
            },
        ];
        assert_eq!(
            rows(&repos, &[]),
            [
                Row::Repo(0),
                Row::File { repo: 0, file: 0 },
                Row::StashHeader(0),
                Row::StashFile { repo: 0, file: 0 },
                Row::StashFile { repo: 0, file: 1 },
                Row::Repo(1),
                Row::Note(1),
                Row::StashHeader(1),
            ],
            "a clean repo can still have a stash; a collapsed stash shows only its header"
        );
        repos[0].collapsed = true;
        assert_eq!(
            rows(&repos, &[RepoId(1)]),
            [Row::Repo(0)],
            "collapsing the repo hides the stash too"
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
                stash_files: 0,
                stash_collapsed: false,
                operation: false,
            },
            RepoRows {
                id: RepoId(2),
                status: None,
                collapsed: false,
                failed: true,
                stash_files: 0,
                stash_collapsed: false,
                operation: false,
            },
        ];
        assert_eq!(
            rows(&repos, &[]),
            [Row::Repo(0), Row::Repo(1), Row::Note(1)]
        );
    }

    #[test]
    fn an_unfinished_operation_gets_a_banner_under_the_header() {
        let fe = status(vec![file("a.ts", None, Some(ChangeKind::Conflicted))]);
        let repos = [RepoRows {
            id: RepoId(1),
            status: Some(&fe),
            collapsed: false,
            failed: false,
            stash_files: 0,
            stash_collapsed: false,
            operation: true,
        }];
        assert_eq!(
            rows(&repos, &[]),
            [
                Row::Repo(0),
                Row::Operation(0),
                Row::File { repo: 0, file: 0 }
            ]
        );
    }

    #[test]
    fn arrow_keys_step_over_headers_to_the_next_file() {
        let rows = [
            Row::Repo(0),
            Row::File { repo: 0, file: 0 },
            Row::File { repo: 0, file: 1 },
            Row::Repo(1),
            Row::Note(1),
            Row::File { repo: 2, file: 0 },
        ];
        assert_eq!(step_file(&rows, None, 1), Some((0, 0)));
        assert_eq!(step_file(&rows, None, -1), Some((2, 0)));
        assert_eq!(step_file(&rows, Some((0, 1)), 1), Some((2, 0)));
        assert_eq!(step_file(&rows, Some((2, 0)), 1), Some((2, 0)));
        assert_eq!(step_file(&rows, Some((0, 0)), -1), Some((0, 0)));
        assert_eq!(step_file(&[Row::Repo(0)], None, 1), None);
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
        assert_eq!(commit_targets(&repos, &[]), [RepoId(1), RepoId(3)]);
        assert_eq!(commit_targets(&repos, &[RepoId(3)]), [RepoId(3)]);
        assert!(commit_targets(&repos, &[RepoId(2)]).is_empty());
        assert_eq!(
            commit_targets(&repos, &[RepoId(1), RepoId(3)]),
            [RepoId(1), RepoId(3)]
        );
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

    fn item(hash: &str, unpushed: bool, action: PlanAction) -> PlanItem {
        PlanItem {
            hash: hash.into(),
            summary: hash.into(),
            unpushed,
            action,
        }
    }

    #[test]
    fn actions_cycle_and_the_oldest_row_cannot_squash() {
        let mut plan = vec![
            item("c", true, PlanAction::Pick),
            item("b", true, PlanAction::Pick),
        ];
        let seen: Vec<_> = (0..4)
            .map(|_| {
                cycle_action(&mut plan, 0);
                plan[0].action.label()
            })
            .collect();
        assert_eq!(seen, ["Squash", "Fixup", "Drop", "Pick"]);
        cycle_action(&mut plan, 1);
        assert_eq!(
            plan[1].action,
            PlanAction::Drop,
            "nothing below to squash into"
        );
        cycle_action(&mut plan, 9);
    }

    #[test]
    fn rows_move_within_bounds_and_the_plan_is_built_oldest_first() {
        let mut plan = vec![
            item("c", true, PlanAction::Pick),
            item("b", true, PlanAction::Squash),
        ];
        assert_eq!(move_item(&mut plan, 0, true), 0);
        assert_eq!(move_item(&mut plan, 1, false), 1);
        assert_eq!(move_item(&mut plan, 0, false), 1);
        assert_eq!(plan[0].hash, "b");
        let entries = plan_entries(&plan);
        assert_eq!(entries[0].hash, "c");
        assert_eq!(entries[1].action, devspace_git::RebaseAction::Squash);
    }

    #[test]
    fn a_rewrite_that_reaches_pushed_commits_is_flagged() {
        let original: Vec<String> = ["c", "b", "a"].map(String::from).to_vec();
        let plan = |actions: [PlanAction; 3]| -> Vec<PlanItem> {
            vec![
                item("c", true, actions[0].clone()),
                item("b", true, actions[1].clone()),
                item("a", false, actions[2].clone()),
            ]
        };
        use PlanAction::*;
        let untouched = plan([Pick, Pick, Pick]);
        assert!(!plan_changes_history(&untouched, &original));
        assert!(!plan_touches_pushed(&untouched, &original));
        let local_only = plan([Squash, Pick, Pick]);
        assert!(plan_changes_history(&local_only, &original));
        assert!(
            !plan_touches_pushed(&local_only, &original),
            "only unpushed rows change"
        );
        let reaches = plan([Pick, Squash, Pick]);
        assert!(plan_touches_pushed(&reaches, &original)); // joins the pushed commit below it
        let touches = plan([Pick, Pick, Drop]);
        assert!(plan_touches_pushed(&touches, &original));
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
