# Notes

Notes are Markdown, stored as plain `.md` files in the project's folder, so you can read them with any other tool. There are two kinds: notes that belong to the project, and the description of a task (see [Tasks and subtasks](topic:tasks)).

## Working with notes

- **Create.** Add a note from the Notes list on the project page. Rename or delete it from the same list.
- **Open.** A note that has content opens in **Preview**. An empty one opens in **Edit** so you can start typing.
- **Edit, Split, Preview.** The toggle at the top switches between the source, source beside the result, and the result alone. Double-click the rendered note to start editing it.
- **Saving.** Every change is saved as you type. `Ctrl+S` saves and goes back to Preview. Escape in the editor leaves without keeping the edits of that session.
- **Toolbar.** Buttons for bold, italic, heading, inline code, code block, SQL block, table, lists, task list, quote, link and divider. They wrap the selection or insert a block.

## In the preview

- Select text with the mouse and copy it with `Ctrl+C`. `Ctrl+A` selects everything and `Ctrl+F` searches the note, with `F3` and `Shift+F3` for the next and previous match.
- Code blocks have a header with the language and a **Copy** button. Common languages are coloured; SQL gets a full set of keywords.
- Click a task checkbox to tick or untick it. The note's text changes with it.
- Links open in your browser. Only `http`, `https` and `mailto` links are opened. A `#heading` link scrolls to that heading.

The full list of what the preview understands, with examples, is in the [Markdown reference](topic:markdown). Shown inside this guide it is read-only: its checkboxes and references do nothing here. Select an example and copy it into a note to try it.

## References to notes and tasks

A note can point at other notes and tasks of the same project:

- `[[Title]]` links a note or task by its title. `[[Title|label]]` shows your own label. A subtask can be written with its parents: `[[Release › Deploy › Plan]]`.
- `#12` links the task with that number (the small number beside its title).

Click a reference to open it. One that finds its target is drawn as a tinted chip. A `[[Title]]` that finds nothing is muted with a wavy underline, so you can see it is broken before you click it. A `#12` that is no task of this project is left as plain text, so numbers in prose do not turn into broken links.

While you type, `[[` opens a list of the project's notes and tasks, and `#` (after a space or an opening bracket) opens a list of its tasks. Up and Down move, Enter or Tab or a click inserts the choice, Escape closes the list. Picking a subtask writes its whole chain, so the link opens that one and not another task with the same name.

## Slash commands

In the editor, type `/` at the start of a line to open a list of blocks to insert: heading, table, code and SQL blocks, lists, task list, quote, alerts, link and divider. Keep typing to narrow the list, then press Enter or Tab. The list stays closed in the middle of a line, in a path like `/usr/bin`, and inside a code block. The full table of commands is in the [Markdown reference](topic:markdown).

## Alerts

`> [!NOTE]`, `> [!TIP]`, `> [!IMPORTANT]`, `> [!WARNING]` and `> [!CAUTION]` draw a callout, as on GitHub. This app adds `> [!TODO]`, `> [!BUG]` and `> [!QUESTION]`. A `-` after the kind (`> [!NOTE]-`) starts the alert folded, and a `+` starts it open; click its header to flip it. GitHub shows our three extra kinds and the fold markers as plain quotes.

## Finding notes

The [command palette](topic:search) searches note titles in every project, and the text inside notes (it shows an excerpt of the match).
