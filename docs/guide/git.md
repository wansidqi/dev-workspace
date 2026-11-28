# Git

Git is one entry in the sidebar with two tabs: **Changes** (`Ctrl+Shift+G`) and **History** (`Ctrl+Shift+H`). Unlike a single-repository client, every view covers **all the repositories linked to the project** at once, grouped and labelled by repository, with a repository filter to narrow to one.

A commit still belongs to one repository, so "commit" across several means one commit in each selected repository. Branch switches, fetch, pull, push and stash also run once per repository and report one result line per repository. One failing repository never stops the others.

## Changes

The working copy of each repository: staged, unstaged, untracked and conflicted files.

- **Diffs.** Click a file to see its diff. Large, binary and non-text files are flagged instead of drawn.
- **Stage, unstage, discard.** By file, by hunk or by single line. Discard keeps a copy of the files first in the repository's `.git/devspace-discarded` folder, so it can be undone. Discard always asks first.
- **Commit.** Summary, description and optional co-authors. Amend works on one repository at a time (the filtered one). A commit across repositories uses the same message and each repository's own staged files.
- **Branches.** Create, switch, rename and delete from the repository's branch popover. Switching with uncommitted changes asks whether to carry them or stash them. Deleting a branch asks first.
- **Stash.** Each branch has one stash, a "shelf" that can hold any number of files. Stash a whole repository (the down-arrow on its header) or a single file (the down-arrow on its row). A **Stashed** section under the repository lists the files; tick some to restore or discard just those, or use **Restore all**. Restoring merges with what you changed since, so nothing is overwritten, and a file that would conflict stays in the shelf and is reported. This is not a `git stash`: `git stash list` and other git tools do not show it, and stashes made by those tools are not shown here.
- **Tie to a task.** Choose a task in **Link commit to a task** before committing and the commits are linked to it in every repository. If the task has a branch, **Use task branch in all repos** switches every repository to it, creating it where it is missing.
- **Staying current.** The view refreshes after each action, on **Refresh**, and when the window regains focus, so edits made in your IDE or terminal show up.

## Fetch, pull, push

The buttons in the header run on every shown repository, a few at a time. A sync button on each repository does fetch, pull and push in one go, and shows how far ahead or behind you are.

- Pull follows your own git settings (`pull.rebase`, `pull.ff`). Push publishes a new branch the first time. A rejected push offers a safe force push (`--force-with-lease`).
- **Clone** is in the header: enter the address, pick a folder, and the clone is linked to the project.
- Settings can turn on a background fetch every 5 minutes. It is off by default. See [Settings and data](topic:settings).
- These actions use your own `git`, so your credential manager, SSH setup and hooks apply. If git is not installed, the app says so.

## Merge, conflicts, undo

- **Merge** a branch into the current one from the history popover. A preview first shows the commits to merge and which files would conflict, without touching anything.
- An unfinished merge, rebase, cherry-pick or revert shows a banner with **Continue**, **Skip** (rebase and cherry-pick) and **Abort** (asks first).
- A conflicted file gets **Mine**, **Theirs** and **Mark resolved**. Mark resolved refuses while conflict markers are still in the file. Double-click opens the file in your IDE.
- **Revert** makes a new commit that undoes one, keeping history. **Undo last commit** is a soft reset that puts the message back in the commit box; it asks first and says if the commit was already pushed.

## History

A single list of commits from all linked repositories, newest first, each row labelled with its repository. Search it and filter by repository, branch, author or date.

- **Map commits to tasks.** Select one commit, several, or a range, and map them to any task or subtask of the same project. A task can have many commits and a commit can belong to several tasks. Each link has its own note and a status (Linked, Needs migration, Cherry-picked, Deployed) shown as a badge on the commit.
- **Unassigned only** shows commits not mapped to any task yet.
- **Suggested commits.** On a task's **Commits** tab, commits whose message mentions the task number (`#12`) or whose branch is the task's branch are suggested, with one-click mapping.
- **Branch view.** For a task with a branch, its Commits tab lists the commits on that branch that are not in the target branch.
- **Copy cherry-pick list.** Copies the task's commits in order, ready for a cherry-pick.
- **Missing commits.** After a rebase or squash the app follows a mapped commit to its rewritten version by its patch. If it cannot find one, the link is flagged as Missing instead of silently dropped.

## Rewriting history

- **Interactive rebase** of the newest 30 commits: squash, fixup, drop, reorder and reword. It needs a clean working copy and warns when you rewrite commits that were pushed.
- **Cherry-pick.** Tick commits and press **Cherry-pick**, or drag a commit onto a branch. They are applied oldest first, stopping on a conflict.

## GitHub (optional)

Off by default, and nothing contacts GitHub while it is off. In [Settings and data](topic:settings) you can turn it on and sign in. Then a repository whose remote is on GitHub lets you list and create pull requests and shows its CI status beside the branch. The token is kept in the system keychain.
