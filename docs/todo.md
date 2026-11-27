# Todo

_Added 2026-11-27_

Planned work that is not built yet. Finished work is recorded in [progress.md](progress.md).

## Markdown: more customization

The note preview (`crates/app/src/markdown.rs`, `markdown_view.rs`) already covers GitHub's basic writing syntax, including alerts. These are the next ideas, ordered by how well they fit the workspace.

Rules that still apply to all of them: notes are untrusted input (raw HTML stays inert, never interpreted), parsing stays pure and unit-tested in `markdown.rs`, styling uses theme tokens, and anything that needs a new crate is raised before it is added.

### High value, fits the app

- [x] **Task and note references.** Two syntaxes, built in two steps. Both render as clickable chips that open the target, and both are **same project only**. Done 2026-11-27; the remaining notes below are gaps.
  - [x] Step 1a (2026-11-27): `[[Title]]` and `[[Title|label]]` render as links in note previews and task descriptions; a click opens the project note, or else the task, with that title (case-insensitive, a note wins over a task). A missing title shows a notice. Code in `markdown.rs` (`split_refs`), `refs.rs` (resolver), `note_page.rs` and `task_detail.rs` (click handlers).
  - [x] Step 1b (2026-11-27): typing `[[` in the note editor opens a popup under the caret listing the project's notes and tasks, filtered as you type (prefix, then word, then contains). Up/Down move, Enter or Tab (or a click) insert `[[Title]]`, Escape closes the popup first. Logic in `refs.rs` (`open_query`, `filter`, `candidates`), popup in `note_editor.rs`. Subtasks show "in Parent › Child" in the popup and are written with their chain (`[[Release › Deploy › Plan]]`), so a link finds the task you picked and not a namesake; a bare `[[Plan]]` prefers the top-level task, and a chain may start lower down (`[[Deploy › Plan]]`). Typing a parent's name and `›` or `>` lists only its subtasks. Tried by hand in the app: popup and rendered link confirmed.
  - [x] Step 1c (2026-11-27): a reference that resolves is drawn as a tinted chip (not an underlined web link); one that does not is muted with a wavy underline in the danger colour, before any click. The look is decided per draw by a lookup in `refs.rs` (`look`, `lookup`), built only for a note that mentions references. Drawing in `markdown_view.rs` (`inline`).
  - [x] Step 2 (2026-11-27): `#4` links a task by its number, the same one the task page shows next to the title and that commit messages use. In the preview it reads `#4 Task title` (struck through and dimmed when the task is Done). A `#N` that is no task of the open project is left as plain text, so issue or PR numbers in prose stay quiet. Typing `#` after other text opens a task picker (by number, else by title); a heading `# Title`, `C#` or a finished `#4` never opens it. `[[#4]]` works too. Parsing in `markdown.rs` (`split_task_numbers`), rules in `refs.rs`, picker in `note_editor.rs`. Not yet tried by hand in the app.
  - Task numbers are the tasks' database ids, so they are unique across all projects, not only within one. A reference only resolves against the open project's tasks. The GitHub issue number shown in the `#N` chip on a linked task is a different number.
  - Known gap: the status shown for `#4` is only Done (struck through); Todo, In progress and Blocked look the same.
  - Titles resolve from the project's notes and tasks already held in `AppState`, so no database query or background pass is needed.
  - Cross-project (`[[Project name/Title]]`) is out of scope for now.
- [ ] **Commit references.** A short hash in backticks or `repo@hash` renders as a chip that opens the commit in History. Identify a commit by (repo_id, hash), never by hash alone. Optionally offer "link to this task".
- [ ] **Code block extras.** Line numbers, a filename title (`` ```rust title="main.rs" ``), highlighted lines, and diff colouring for `diff` blocks (reuse the git diff colours).
- [ ] **Collapsible sections.** Render `<details><summary>` natively with a chevron. Every other raw HTML tag stays plain text.
- [ ] **Mermaid diagrams.** Decide first: a Rust renderer (new dependency, costly) or a styled source block with an "Open in Lucid" button (cheap). Ask before adding a crate.
- [ ] **Math.** `$...$` and `$$...$$`. Low priority unless formulas are needed. Needs a dependency decision.

### Reading comfort

- [ ] **Table of contents rail.** Outline built from headings, click to jump. Check how `#anchor` links already scroll before building it.
- [ ] **Image sizing, captions and zoom.** `![alt](path =300x)`, click to enlarge, paths relative to the project's attachments folder. Paths must be validated and stay inside the project directory.
- [ ] **More alert kinds.** Custom kinds such as `[!TODO]`, `[!BUG]`, `[!QUESTION]`, and collapsible alerts. Extend `AlertKind`, give each a Lucide icon and a status colour (shape plus colour, never colour alone).

### Productivity

- [x] **Editor slash commands and snippets** (2026-11-27). `/` at the start of a line opens a popup of blocks (heading, table, code, SQL, task list, bulleted and numbered list, quote, the five alert callouts, link, divider); typing narrows it by name or keyword (`/alert` lists every callout, `/todo` finds the task list). Enter, Tab or a click inserts it; Escape closes it. A format makes the same edit as the toolbar button, so the two agree; a callout inserts `> [!NOTE]` and leaves the caret inside. It stays closed for paths (`/usr/bin`), mid-line slashes, indented lines and fenced code. Logic in `slash.rs`, popup in `note_editor.rs`. The toolbar for bold, link and checkbox already existed. Not yet tried by hand in the app.
  - Gap: the popup lists names only; there is no preview of the block, and snippets of your own are not supported.
- [ ] **Frontmatter.** A YAML header (`tags`, `status`, `due`) shown as a properties strip above the note. The notes list can then filter on those fields. Decide whether it is display-only or also stored in SQLite (a schema change needs a numbered migration and approval).

### Not planned

- **Credential mentions in notes** (for example `{{cred:db-prod}}`). It would put secret references into plain markdown files and blur the line with the encrypted vault. Revisit only with a clear security design.

## Suggested order

1. Task and note references
2. Commit references
3. Code block extras

They reuse data the app already has and need no new dependencies.
