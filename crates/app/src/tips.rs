//! Tooltip texts, keyed by the name of a control's element id.
//!
//! The shared buttons, icon buttons, checkboxes, segments and chips in
//! `ui.rs` look their id up here and show the tip on hover, so the wording for
//! a whole screen lives in one place and a newcomer can learn the screen by
//! pointing at things. Ids with a number suffix (`("file-check", 7)`) match by
//! their name. Add a line here to give a control a tooltip.

/// `(title, what-it-does)` for the control with this id name.
pub fn lookup(name: &str) -> Option<(&'static str, Option<&'static str>)> {
    let tip = |title, hint| Some((title, Some(hint)));
    match name {
        // ---- Changes: header ---------------------------------------------
        "changes-back" => Some(("Back to the project", None)),
        "changes-all-branches" => tip(
            "Switch or create a branch in every repo",
            "Applies to all the project's repos at once. To change just one repo, use the branch button on its row.",
        ),
        "changes-refresh" => tip(
            "Re-read the repositories",
            "This also happens by itself whenever you come back to this window.",
        ),
        "changes-notice-dismiss" => Some(("Dismiss this message", None)),
        "changes-repo-all" => Some(("Show the changes of every linked repo", None)),
        "changes-repo-filter" => tip(
            "Show only this repo",
            "Narrows the list, and a commit then goes only to this repo.",
        ),

        // ---- Changes: a repo's row and its files -----------------------------
        "repo-collapse" => Some(("Collapse or expand this repo's files", None)),
        "repo-check" => tip(
            "Stage or unstage every file in this repo",
            "Staged files go into the next commit. A dash means only some files are staged.",
        ),
        "repo-branch" => tip(
            "This repo's branch",
            "Click to switch, create, rename or delete branches in this repo. ↑ means commits not pushed yet, ↓ commits you don't have yet.",
        ),
        "repo-stash-now" => tip(
            "Stash all changes in this repo",
            "Moves every change, new files included, into this branch's stash and cleans the working copy. Bring them back later from the Stashed section.",
        ),
        "repo-discard" => tip(
            "Discard all changes in this repo",
            "Throws away every uncommitted change. You are asked first, and a backup copy is kept in .git/devspace-discarded.",
        ),
        "file-check" => tip(
            "Include in the next commit",
            "Tick to stage the file. A dash means only part of it is staged.",
        ),
        "file-stash" => tip(
            "Stash this file",
            "Moves just this file's changes into the branch's stash so the rest of your work stays.",
        ),
        "file-open" => tip(
            "Open in your IDE",
            "Double-clicking the row does the same. The IDE command is in Settings.",
        ),
        "file-discard" => tip(
            "Discard changes to this file",
            "You are asked first, and a backup copy is kept.",
        ),

        // ---- Changes: the stash ------------------------------------------------
        "stash-collapse" => Some(("Collapse or expand the stashed files", None)),
        "stash-restore" => tip(
            "Put stashed files back",
            "Restores the ticked files, or all of them if none is ticked. Edits you made since are kept; a file that would conflict stays in the stash.",
        ),
        "stash-discard" => tip(
            "Delete stashed files",
            "Permanently removes the ticked files from the stash, or all of them if none is ticked. You are asked first.",
        ),
        "stash-check" => tip(
            "Select to restore or delete only some files",
            "Then use Restore or the bin on the Stashed row.",
        ),
        "stash-restore-one" => tip(
            "Restore this file",
            "Puts it back in your working copy and removes it from the stash.",
        ),
        "stash-diff-restore" => tip(
            "Restore this file",
            "Puts it back in your working copy and removes it from the stash.",
        ),
        "stash-diff-discard" => tip(
            "Delete this file from the stash",
            "You are asked first.",
        ),

        // ---- Changes: reading and staging a diff -------------------------------
        "side-unstaged" => tip(
            "Changes not staged yet",
            "What you edited since the last stage. Staging moves it to the other tab.",
        ),
        "side-staged" => tip(
            "Changes that will be committed",
            "These go into the next commit.",
        ),
        "hunk-stage" => tip(
            "Stage or unstage this block",
            "A block is one run of nearby changes. Only this block moves.",
        ),
        "hunk-discard" => tip(
            "Discard this block",
            "Throws away just this block of changes. You are asked first.",
        ),
        "line-check" => tip(
            "Select this line",
            "Shift-click another line to select everything between. Then stage, unstage or discard the selection.",
        ),
        "lines-stage" => tip(
            "Stage or unstage the selected lines",
            "Only the lines you ticked, so one file can go into two commits.",
        ),
        "lines-discard" => tip(
            "Discard the selected lines",
            "Throws away only those lines. You are asked first.",
        ),
        "lines-clear" => Some(("Clear the selection", None)),

        // ---- Changes: commit box ------------------------------------------------
        "changes-task-link" => tip(
            "Link new commits to a task",
            "Commits you make now are attached to this task in every repo, so they show on its Commits tab.",
        ),
        "changes-amend" => tip(
            "Amend the last commit",
            "Replaces the previous commit instead of adding a new one. Don't amend a commit you already pushed.",
        ),
        "changes-commit" => tip(
            "Commit the staged files",
            "Makes one commit in each repo that has staged files, all with the same message. Tick files on the left to stage them.",
        ),

        // ---- Changes: popovers ---------------------------------------------------
        "popover-close" => Some(("Close", None)),
        "mode-bring" => tip(
            "Bring your changes along",
            "Your uncommitted edits move to the new branch. If one would be overwritten, nothing happens and you are told.",
        ),
        "mode-stash" => tip(
            "Stash your changes first",
            "They are saved in the current branch's stash, then the branch is switched. Restore them when you come back.",
        ),
        "branch-rename" => Some(("Rename this branch", None)),
        "branch-delete" => tip(
            "Delete this branch",
            "If it has commits that aren't in your current branch, you are asked to confirm.",
        ),
        "use-task-branch" => tip(
            "Switch every repo to the task's branch",
            "Creates it in the repos that don't have it yet.",
        ),
        "confirm-yes" => Some(("Do it", None)),
        "confirm-no" => Some(("Cancel, nothing is changed", None)),
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
    fn every_tip_has_a_short_title() {
        // The tooltip is a glance, not a paragraph.
        for name in [
            "changes-back",
            "repo-branch",
            "file-stash",
            "stash-restore",
            "line-check",
            "changes-commit",
            "mode-stash",
        ] {
            let (title, hint) = lookup(name).unwrap();
            assert!(title.len() <= 50, "{name}: {title}");
            if let Some(h) = hint {
                assert!(h.len() <= 190, "{name}: hint is {} chars", h.len());
            }
        }
    }
}
