# Todo

_Added 2026-11-27_

Planned work that is not built yet. Finished work is recorded in [progress.md](progress.md).

## Markdown: more customization

The note preview (`crates/app/src/markdown.rs`, `markdown_view.rs`) already covers GitHub's basic writing syntax, including alerts. These are the next ideas, ordered by how well they fit the workspace.

Rules that still apply to all of them: notes are untrusted input (raw HTML stays inert, never interpreted), parsing stays pure and unit-tested in `markdown.rs`, styling uses theme tokens, and anything that needs a new crate is raised before it is added.

### High value, fits the app

- [ ] **Task and note references.** Two syntaxes, built in two steps. Both render as clickable chips that open the target, and both are **same project only**.
  - [ ] Step 1: `[[Title]]` links a note or a task by title. Typing `[[` opens an autocomplete list of the project's notes and tasks and inserts the chosen title.
  - [ ] Step 2: `#4` links a task by number (the same convention the git code already uses to suggest commits for a task). Typing `#` opens a task picker. The chip shows the task title and status. Notes have no number, so `#` is for tasks only.
  - Check first whether task numbers are visible in the UI. If not, show the number on the task page before step 2, or ship only `[[...]]`.
  - Resolve titles in one background pass per note and cache the result; never query the database on the UI thread.
  - A deleted or missing target renders as a muted "not found" chip.
  - Cross-project (`[[Project name/Title]]`) is out of scope for now. Task numbers are only unique within a project, so `#4` never crosses projects.
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

- [ ] **Editor slash commands and snippets.** Typing `/table`, `/alert`, `/code` inserts a template. Add a small toolbar for bold, link and checkbox.
- [ ] **Frontmatter.** A YAML header (`tags`, `status`, `due`) shown as a properties strip above the note. The notes list can then filter on those fields. Decide whether it is display-only or also stored in SQLite (a schema change needs a numbered migration and approval).

### Not planned

- **Credential mentions in notes** (for example `{{cred:db-prod}}`). It would put secret references into plain markdown files and blur the line with the encrypted vault. Revisit only with a clear security design.

## Suggested order

1. Task and note references
2. Commit references
3. Code block extras

They reuse data the app already has and need no new dependencies.
