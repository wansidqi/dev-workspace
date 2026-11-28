# Tasks and subtasks

A task is a unit of work. It can have subtasks, and they can have subtasks, to any depth. A task works with no git and no GitHub; both are optional.

## The task tree

On the project page tasks form a tree. A subtask hangs off its parent's status icon with a rounded connector line.

- **Open a task.** Click its row.
- **Change its status.** Click the status icon. It cycles To do, In progress, Done, then Blocked, then back to To do. Status is also shown by the icon's shape, not only its colour.
- **Fold subtasks.** The chevron after a title folds that task's subtasks and shows a count. **Collapse all** and **Expand all** are in the Tasks header.
- **Filter by tag.** The tag row filters the list. A filtered list is flat, because a parent may be hidden.
- **Actions.** Buttons appear over the right edge of a row when you hover it.

## The task page

The page shows a breadcrumb of its parents (Back goes to the parent), the title, the status, and its properties:

- **Priority.** Click to cycle Low, Medium, High, Urgent.
- **Due date.** Click to edit, as `YYYY-MM-DD`. An empty value clears it. Unfinished tasks past their date show **Overdue**.
- **Branch.** The git branch for this task. Click to edit, blank clears. It feeds the branch view and suggested commits (see [Git](topic:git)).
- **Tags.** Add with **Add tag** and Enter. A tag is trimmed, at most 32 characters, cannot contain commas, and is unique on a task ignoring case.

Under the properties are tabs:

- **Description.** The task's body, written in [Markdown](topic:markdown). **Edit** opens it in the note editor. Checkboxes in it can be ticked directly.
- **Subtasks.** The task's children.
- **Attachments.** Files for this task. See [Attachments](topic:attachments).
- **Commits.** Git commits mapped to this task. See [Git](topic:git).
- **Comments.** Only for a task that is a GitHub issue.

## Task numbers

Every task has a number, shown small beside its title as `#12`. Commit messages that mention it (for example `fix login #12`) make those commits show up as suggestions on the task. You can also write the number in a note to link to the task. See [Notes](topic:notes).

The number is not a GitHub issue number. A task that is linked to an issue shows the issue number in its own chip.

## Deleting

Deleting a task deletes its subtasks, notes, attachments, commit links and tags, and the files behind them.

## GitHub issues (optional)

With the GitHub integration on in [Settings](topic:settings) and a linked repository on GitHub, open issues come in as tasks: sub-issues become subtasks, labels become tags, the body becomes the description and a closed issue becomes Done. Edits to a linked task (title, status, tags, description) are sent back after a short delay. GitHub is the source of truth for a linked task.

Publishing a local task as an issue is always an explicit click on **Publish to GitHub**. GitHub allows 8 levels and 100 children per parent, so deeper local tasks cannot sync. Deleting a linked task does not close its issue, and the next refresh brings it back.
