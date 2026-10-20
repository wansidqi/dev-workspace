//! Tooltip texts, keyed by the name of a control's element id.
//!
//! The shared buttons, icon buttons, checkboxes, segments and chips in
//! `ui.rs` look their id up here and show the tip on hover, so the wording for
//! a whole screen lives in one place and a newcomer can learn the screen by
//! pointing at things. Ids with a number suffix (`("file-check", 7)`) match by
//! their name. Add a line here to give a control a tooltip.
//!
//! Keep them short: a title of a few words and at most one plain sentence.

/// `(title, what-it-does)` for the control with this id name.
pub fn lookup(name: &str) -> Option<(&'static str, Option<&'static str>)> {
    let tip = |title, hint| Some((title, Some(hint)));
    match name {
        // ---- Changes: header ---------------------------------------------
        "changes-back" => Some(("Back to the project", None)),
        "changes-all-branches" => tip(
            "Branches for all repos",
            "Switch or create a branch in every repo at once.",
        ),
        "changes-refresh" => tip(
            "Refresh",
            "Re-reads every repo; this also happens when you return here.",
        ),
        "changes-notice-dismiss" => Some(("Dismiss", None)),
        "changes-repo-all" => Some(("Show every repo", None)),
        "changes-repo-filter" => tip(
            "Show only this repo",
            "A commit then goes to this repo only.",
        ),

        // ---- Changes: a repo's row and its files -----------------------------
        "repo-collapse" => Some(("Collapse or expand", None)),
        "repo-check" => tip(
            "Stage or unstage all",
            "Staged files go into the next commit.",
        ),
        "repo-branch" => tip(
            "Change branch",
            "Switch, create, rename or delete branches in this repo.",
        ),
        "repo-stash-now" => tip(
            "Stash all changes",
            "Sets every change aside for this branch; restore from Stashed.",
        ),
        "repo-discard" => tip(
            "Discard all changes",
            "Throws away every change in this repo; you are asked first.",
        ),
        "file-check" => tip(
            "Include in the commit",
            "A dash means only part of the file is staged.",
        ),
        "file-stash" => tip(
            "Stash this file",
            "Set just this file aside; the rest stays.",
        ),
        "file-open" => tip("Open in IDE", "Double-clicking the file does the same."),
        "file-discard" => tip("Discard this file's changes", "You are asked first."),

        // ---- Changes: the stash ------------------------------------------------
        "stash-collapse" => Some(("Collapse or expand", None)),
        "stash-restore" => tip(
            "Restore stashed files",
            "Puts back the ticked files (all if none), keeping newer edits.",
        ),
        "stash-discard" => tip(
            "Delete from the stash",
            "Removes the ticked files (all if none); you are asked first.",
        ),
        "stash-check" => tip("Select", "Pick the files to restore or delete."),
        "stash-restore-one" => tip(
            "Restore this file",
            "Puts it back and removes it from the stash.",
        ),
        "stash-diff-restore" => tip(
            "Restore this file",
            "Puts it back and removes it from the stash.",
        ),
        "stash-diff-discard" => tip("Delete from the stash", "You are asked first."),

        // ---- Changes: reading and staging a diff -------------------------------
        "side-unstaged" => tip(
            "Not staged yet",
            "Edits that won't be committed until you stage them.",
        ),
        "side-staged" => tip("Staged", "These go into the next commit."),
        "hunk-stage" => tip(
            "Stage or unstage this block",
            "Moves only this group of changes.",
        ),
        "hunk-discard" => tip("Discard this block", "You are asked first."),
        "line-check" => tip("Select line", "Shift-click to select a range."),
        "lines-stage" => tip(
            "Stage or unstage the lines",
            "Only the selected lines move.",
        ),
        "lines-discard" => tip("Discard the selected lines", "You are asked first."),
        "lines-clear" => Some(("Clear selection", None)),

        // ---- Changes: commit box ------------------------------------------------
        "changes-task-link" => tip(
            "Link to a task",
            "New commits show on the task's Commits tab.",
        ),
        "changes-amend" => tip(
            "Amend last commit",
            "Replaces the previous commit; not for ones already pushed.",
        ),
        "changes-commit" => tip(
            "Commit staged files",
            "One commit in each repo that has staged files.",
        ),

        // ---- Changes: popovers ---------------------------------------------------
        "popover-close" => Some(("Close", None)),
        "mode-bring" => tip(
            "Bring your changes",
            "They move to the new branch, unless one would be overwritten.",
        ),
        "mode-stash" => tip(
            "Stash them first",
            "They wait in this branch's stash until you come back.",
        ),
        "branch-rename" => Some(("Rename", None)),
        "branch-delete" => tip(
            "Delete branch",
            "Asks first if it has commits that aren't merged.",
        ),
        "use-task-branch" => tip(
            "Use the task's branch",
            "Switches every repo, creating it where missing.",
        ),
        "confirm-yes" => Some(("Do it", None)),
        "confirm-no" => Some(("Cancel", None)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_have_tips_and_unknown_ones_do_not() {
        assert!(lookup("repo-stash-now").is_some());
        assert!(lookup("some-other-screen-button").is_none());
    }

    #[test]
    fn tips_stay_short() {
        // A tooltip is a glance: a few words, and at most one short sentence.
        for name in [
            "changes-all-branches",
            "changes-refresh",
            "changes-repo-filter",
            "repo-check",
            "repo-branch",
            "repo-stash-now",
            "repo-discard",
            "file-check",
            "file-stash",
            "file-open",
            "file-discard",
            "stash-restore",
            "stash-discard",
            "stash-restore-one",
            "side-unstaged",
            "hunk-stage",
            "line-check",
            "lines-stage",
            "changes-task-link",
            "changes-amend",
            "changes-commit",
            "mode-bring",
            "mode-stash",
            "branch-delete",
            "use-task-branch",
        ] {
            let (title, hint) = lookup(name).unwrap();
            assert!(title.len() <= 32, "{name}: title `{title}`");
            let hint = hint.unwrap_or_else(|| panic!("{name} has no hint"));
            assert!(hint.len() <= 80, "{name}: hint is {} chars", hint.len());
            assert!(!hint.contains('/'), "{name}: no paths in tooltips");
            assert_eq!(hint.matches('.').count(), 1, "{name}: one sentence");
        }
    }
}
