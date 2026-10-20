# CLAUDE.md

Guidance for Claude when working in this repository.

## Project overview

A local-first **developer workspace** desktop app. It organizes work around projects, tasks, notes, attachments, credentials and git commits, and launches the developer's existing IDE and terminal. It is the layer around the IDE, not an IDE replacement.

Goals:
- Low memory and CPU use (the reason for Rust + GPUI)
- Local-first: no account, no network required (remote git operations and the optional GitHub integration are the only network features, and the app stays fully usable without them)
- Plain files for user content (markdown, attachments) so data stays portable
- Secure handling of credentials
- A modern, glassy UI (translucent layered surfaces, soft light, depth) that stays fast and readable

Non-goals:
- Building a code editor (launch VS Code or another IDE instead)
- Container orchestration
- Team or cloud sync

## Tech stack

| Area | Choice |
| --- | --- |
| Language | Rust (stable, edition 2021 or later) |
| UI | GPUI (pin an exact version or git revision; the API is pre-1.0 and changes) |
| Component helpers | gpui-component (evaluate before writing custom widgets) |
| Database | SQLite via rusqlite (bundled feature), migrations in code |
| Git | Hybrid. git2 (libgit2) for reads and local index work: history, status, diffs, staging (including hunk/line patches), commit, branch create/switch, stash. The system git CLI (resolved once, run with arguments, never through a shell) for network and history-rewriting work: fetch, pull, push, clone, merge, rebase, revert, cherry-pick. This reuses the user's credential manager, SSH config and hooks. |
| Terminal | alacritty_terminal (emulation) + portable-pty (shell process) |
| Markdown | pulldown-cmark for parsing and preview |
| PDF viewing | pdfium-render (or mupdf); render pages to images |
| Secrets | keyring crate for the master key; XChaCha20-Poly1305 (chacha20poly1305 crate) for stored credentials, with zeroize for plaintext |
| Async | smol or GPUI's own executor; avoid pulling in tokio unless a dependency requires it |
| Errors | thiserror in library code, anyhow at app boundaries |
| Logging | tracing + tracing-subscriber |
| Paths | dirs crate for OS data directories |

## Features to implement

1. **Projects**: create, edit, archive, switch; link one or more git repos; dashboard with open tasks, recent notes, recent commits.
2. **Tasks and subtasks**: unlimited nesting; status, priority, due date, tags; optional branch per task; each has notes, attachments and linked commits.
3. **Notes**: markdown at project, task and subtask level; every note is markdown (the stored `kind` column is unused by the UI); live preview with highlighted, copyable code blocks; search. Stored as plain files.
4. **Attachments**: PDFs, images, other files on a project, task or subtask; built-in PDF viewer; copy into workspace or link by path.
5. **Credentials vault**: per project; encrypted; reveal/copy with auto-hide and clipboard auto-clear; optional injection into terminal sessions as environment variables.
6. **Git integration**: commit history browser (search, filter by repo/branch/author/date) that merges history across all repos linked to the project, newest first, each row labeled with its repo; select single, multiple or range and tag/map/label them to any task or subtask at any nesting depth in the same project (a task can have many commits; a commit can be mapped to more than one task; tasks are picked from the project's task tree); per-link note and status (needs migration, cherry-picked, deployed); badges on linked commits; unassigned commits view; branch view (commits on task branch not in target); suggested commits (task ID in message or branch); export ordered cherry-pick list; warn when a linked commit is missing after rebase or squash.
   **Git client (GitHub Desktop parity)**, in phases: (1) working-copy changes view, file diffs, stage/unstage/discard by file, hunk and line, commit (summary, description, co-authors, amend), branch create/switch/rename/delete with a stash-or-carry prompt, stash; (2) fetch/pull/push with ahead/behind counts and a sync button; (3) merge with conflict detection and resolution, revert, undo last commit; (4) interactive rebase: squash, reorder, drag-to-cherry-pick; (5) optional, off by default: GitHub sign-in (OAuth device flow), pull request list and creation, CI check status. Tie it to tasks: commit from a task, create the task's branch, auto-link new commits to the active task.
7. **Developer tools**: integrated terminal with tabs starting in the project folder; launch external IDE on the project folder; per-project run commands with log view.
8. **Search and navigation**: global search; command palette; keyboard shortcuts; restore last project, task and layout on startup.
9. **Data and safety**: backup and export/import of a project; local-first.

## Architecture

Keep a strict separation:

- **core**: domain types and logic. No GPUI imports. Pure Rust, unit-testable.
- **storage**: SQLite access, file layout, migrations, encryption.
- **services**: git, terminal, process runner, IDE launcher, PDF rendering.
- **ui**: GPUI views, components, actions, keybindings. Talks to core and services only through plain function calls or channels.

Suggested layout:

```
.
├── Cargo.toml
├── CLAUDE.md
├── crates/
│   ├── core/        # domain models, rules, no UI
│   ├── storage/     # sqlite, migrations, crypto, file store
│   ├── git/         # git2 wrapper, commit linking, patch-id
│   ├── terminal/    # pty + alacritty_terminal wrapper
│   ├── runner/      # std-only process runner for per-project run commands
│   └── app/         # GPUI app, views, actions, main.rs
├── assets/          # icons, fonts, themes
└── docs/
```

A Cargo workspace is preferred so core, storage and git can be tested without compiling the UI.

## Data and storage

Location (use the dirs crate, never hardcode paths):

```
<data_dir>/devspace/
├── app.db                      # SQLite: metadata
├── projects/<project_id>/
│   ├── notes/                  # markdown files
│   └── attachments/            # copied files
└── backups/
```

Core tables (names are guidance, keep them stable once created):

```
projects(id, name, description, archived, created_at, updated_at)
repos(id, project_id, path, name)
tasks(id, project_id, parent_id NULL, title, status, priority, due_at, branch, created_at, updated_at)
notes(id, project_id, task_id NULL, kind, title, file_path, updated_at)
attachments(id, project_id, task_id NULL, name, file_path, is_linked, created_at)
credentials(id, project_id, label, kind, nonce, ciphertext, created_at)
commit_links(id, task_id, repo_id, hash, patch_id, status, note, created_at)
task_tags(task_id, tag)               # tag is COLLATE NOCASE; primary key (task_id, tag)
run_commands(id, project_id, name, command, cwd)
```

Rules:
- Commit links store only repo and hash (plus patch_id). Message, author and date are read from git at display time.
- Store the repo reference on every commit link from the start; projects can have several repos.
- A commit hash is only unique within a repo: identify a commit by (repo_id, hash), never by hash alone. Different repos in one project can share short hashes.
- Project scoping: a commit link is valid only if the task's project_id equals the repo's project_id. Enforce this in core/storage (not just the UI) and test it. A task may be at any depth of the subtask tree.
- Commit links are many-to-many between tasks and commits: unique on (task_id, repo_id, hash), but the same commit may link to several tasks. Deleting a task deletes its links (cascade); unlinking a repo from a project must handle or warn about its existing links rather than orphaning them silently.
- The commit browser and dashboard must cover every linked repo, not just the first. Page each repo's history and merge by commit time.
- Tags are unique per task ignoring case (the first spelling is kept), at most 32 characters, no commas or control characters. The rule lives in core (`normalize_tag`) and is enforced again in storage.
- Deleting a task cascades to its notes, attachments, commit links and tags in the database, so the code must also delete the files those rows point at (copied attachments, note files) — the database knows nothing about files.
- Every schema change is a numbered migration. Never edit an applied migration.
- Use transactions for multi-step writes. Enable WAL mode and foreign keys.

## Security rules

- Credentials are encrypted at rest. The master key lives in the OS keychain, never on disk in plain text.
- Never log credential values, decrypted or not. Never include them in error messages or panic output.
- Zero sensitive buffers after use where practical (zeroize crate).
- Clipboard copy of a secret must auto-clear after a short timeout.
- Injected terminal environment variables apply to that session only. Avoid writing secrets to shell history.
- Validate and canonicalize all file paths. Attachments and notes must stay inside the project directory.
- Never launch an attached file that could run code (executables, scripts, installers, shortcuts): reveal it in the file manager instead. Open files by passing the path as a process argument, never through a shell.
- Treat git repo contents and attached files as untrusted input.
- Git CLI calls: pass arguments as a list (never a shell string), put `--` before paths, validate ref names with `is_safe_ref_name`, set `GIT_TERMINAL_PROMPT=0` so a missing credential fails instead of hanging, and never log remote URLs that embed credentials.
- Destructive git actions (discard changes, delete branch, force push, reset, undo commit) need an explicit confirmation; prefer recoverable forms (stash before discard, `--force-with-lease` over `--force`).
- GitHub tokens (optional integration) are stored like credentials: in the OS keychain, never on disk in plain text, never logged.

## GPUI guidelines

- Keep views small; split large views into components.
- State lives in models/entities, not in view structs. Update state through GPUI's entity update calls and call notify when it changes so views re-render.
- Never block the UI thread. Run git, disk, PDF rendering and database work on background tasks and send results back to the UI.
- Use actions and keybindings for user commands so everything is reachable from the command palette.
- Virtualize long lists (commit history, notes lists, search results). Do not render thousands of rows.
- Cache rendered PDF pages and drop them when off screen to keep memory low.
- Since GPUI documentation is thin, read the gpui source and the Zed codebase for patterns before inventing new ones. Do not guess APIs; check the pinned version.

## UI design: modern glass concept

The app should feel modern and glassy: translucent layered surfaces, subtle light edges, soft shadows, generous spacing, smooth motion. Readability and performance always win over effect.

### How glass is achieved in GPUI

GPUI has no CSS-style backdrop-filter for individual panels. Achieve the look in two layers:

1. **Window-level backdrop**: ask the OS for a blurred or translucent window background (GPUI exposes a window background appearance option, for example blurred, with OS-specific results: acrylic/Mica on Windows, vibrancy on macOS, compositor blur on some Linux setups). Verify what the pinned GPUI version supports before relying on it.
2. **In-app surfaces**: fake depth with semi-transparent fills, a 1px light border, a faint top-to-bottom gradient, and a soft shadow. Do not try to blur behind each panel.

Always provide a fallback: if the window blur is unavailable, render the same tokens with a more opaque background so the UI still looks intentional.

### Theme tokens

All colors, radii, spacing and shadows come from a central theme. No hardcoded colors in views.

```rust
pub struct GlassTheme {
    // Surfaces, back to front
    pub window_tint: Hsla,        // tint over the OS blur
    pub scrim: Hsla,              // dims the app behind the palette
    pub surface: Hsla,            // chrome: sidebar (alpha about 0.70)
    pub surface_solid: Hsla,      // the canvas content sits on (opaque)
    pub surface_raised: Hsla,     // cards, popovers, toasts (alpha about 0.90)
    pub surface_sunken: Hsla,     // wells: inputs, logs, code, terminal
    pub surface_hover: Hsla,      // row hover wash, drawn over any surface
    // Edges
    pub border_subtle: Hsla,      // hairlines between rows
    pub border_light: Hsla,       // edge of a surface
    pub border_strong: Hsla,      // popovers, hovered controls
    // Content
    pub text_primary: Hsla,
    pub text_secondary: Hsla,     // body text too: 4.5:1
    pub text_muted: Hsla,         // placeholders and fine print only: 3:1
    pub accent: Hsla,             // phosphor lime (dark), deep moss (light)
    pub accent_fg: Hsla,          // text on a solid accent fill
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    // Shape and space
    pub radius_sm: Pixels,        // 6: controls, chips
    pub radius_md: Pixels,        // 8: rows, wells
    pub radius_lg: Pixels,        // 12: canvases, panels, popovers
    pub space_1..space_8: Pixels, // 4, 8, 12, 16, 20, 24, 32
}
```

Type comes from `theme.rs` too: `UI_FONT` (Geist), `CODE_FONT` (Geist Mono), `TERMINAL_FONT` (a system monospace, kept for terminal glyph coverage), the `FS_*` scale and the `TypeScale` helpers (`t_micro`, `t_small`, `t_body`, `t_prose`, `t_heading`, `t_title`, `t_display`, `t_code`, `t_strong`). Fonts are bundled in `assets/fonts` (OFL) and registered by `theme::install_fonts`.

The app icon is `assets/images/logo.png`, embedded as the Windows icon resource by `crates/app/build.rs` (see progress.md).

Provide a dark theme first and a light theme second, switchable at runtime. Dark is the default (the user's choice); "Follow system" is an option.

### Visual rules

- **Layering**: window tint at the back, sidebar and panels above, cards and popovers on top. Higher layers are more opaque and have stronger shadows.
- **Edges**: every glass surface gets a 1px light border; the top edge can be slightly brighter to suggest light from above.
- **Radius**: consistent scale (6 controls, 8 rows and wells, 12 canvases and popovers). Not everything is a card: lists are flat rows on hairlines; cards are for grouped forms and popovers.
- **Spacing**: use a 4px grid. Prefer generous padding over dense layouts.
- **Typography**: Geist for the interface, Geist Mono for data (hashes, counts, dates, code, note source, uppercase section labels). Six sizes (11, 12, 13, 14 prose, 15, 20, 28) and three weights (regular, medium, semibold), always through the `TypeScale` helpers, never raw sizes. Rendered notes use the prose size.
- **Color**: graphite surfaces with one phosphor-lime accent, used for selection, focus, progress and the single primary action of a view. Status colors (success, warning, danger) are muted, not neon, and status is always also a shape (Lucide status icons), never colour alone.
- **Icons**: Lucide line icons (ISC), stroke 2, embedded from `assets/icons` through `ui::Icon`; an SVG does not inherit text colour, so tint it explicitly (`ui::icon`, `ui::icon_group` for hover).
- **Shared primitives** live in `ui.rs` (`button`, `icon_button`, `chip`, `filter_chip`, `segmented`, `switch`, `tab`, `page_header`, `empty_state`, `notice`, `kbd`). Screens compose these instead of styling raw divs; add a primitive when a third screen needs the same thing.
- **Motion**: short, subtle transitions (about 120 to 200 ms, ease-out) for hover, panel open and selection. No long or bouncy animation. Respect a reduced-motion setting.

### Readability rules

- Text must keep sufficient contrast over any background. Aim for WCAG AA (4.5:1 for body text) against the worst-case wallpaper behind the window.
- Content-heavy areas must be more opaque than navigation chrome: the terminal, markdown editor and preview, PDF viewer, commit list and credentials vault use near-solid surfaces. Glass is for chrome (sidebar, title bar, toolbars, popovers, command palette), not for reading areas.
- Secrets screens should never be see-through.
- Test every screen on both a light and a dark wallpaper.

### Performance rules for glass

Translucency and shadows cost GPU time and memory, which conflicts with this project's efficiency goal:
- Limit stacked translucent layers to 3 deep.
- Avoid large soft shadows on long scrolling lists; use a thin border instead.
- Do not animate blur or large translucent areas.
- Reuse theme values; do not allocate per-frame styles.
- Provide a setting to reduce transparency (solid surfaces), also used automatically when the OS requests reduced transparency.

### Layout concept

- Custom title bar blended into the window: brand at the left over the sidebar, a search field in the middle that opens the command palette (it opts out of dragging with `occlude`), and window controls at the right drawn as three small status-light dots (minimise amber, maximise green, close red; the glyph shows on hover). GPUI draws no caption buttons for a transparent title bar, so these are ours; each is marked with `window_control_area` and is a sibling of the drag region, never a child of it.
- Left glass sidebar (248px): the project list (hover or select shows edit and archive), then a Workspace group of icon rows: Commit history, Run commands, Timesheet, Credentials, Settings (shortcut shown on hover).
- The view area is a floating canvas (radius 12, hairline border) beside the terminal canvas; clip children to it, so a view must not paint its own opaque background at the corners.
- Project page: header (name, meta, Open in IDE, New task) over a status strip, then tasks as a tree (subtasks are indented and hang off their parent's status icon with rounded L-shaped connector lines; the trunk continues past rows with later siblings; a hairline separates top-level groups only, not rows inside a branch; the status icon cycles the status, the row opens the task, actions float over the right edge on hover; a tag filter flattens the list and drops the lines) and a rail with notes, files and repositories. The rail sits beside the tasks when the canvas is at least 960px wide and stacks below otherwise; the terminal panel gives up width before the page drops under 520px.
- Task page: breadcrumb, title, status, properties (priority, due, branch), tags, then underline tabs for Notes, Attachments and Commits.
- Terminal: a collapsible right-hand panel with tabs, one tab set per project (Ctrl+` toggles, Ctrl+Shift+` new tab). Drag its left edge to resize, double-click a tab to rename it, "Split" stacks a second shell in the tab.
- Commit history browser: a full-content view opened from the sidebar, with a mapping side panel. Credentials: a full-content view from the sidebar (add, reveal with auto-hide, copy with clipboard auto-clear, replace, delete).
- Command palette (Ctrl+K or Ctrl+Shift+P) as a centered floating glass popover with a soft shadow; searches commands, projects, tasks, note titles, note text and commits. Shortcuts: Ctrl+Shift+H history, R run commands, M timesheet, L credentials, N new project, Ctrl+, settings.
- Run commands: a sidebar row opens a full-content view with saved commands and a live log. Open in IDE is a project page action; the Settings view (Ctrl+,) holds appearance, terminal scrollback, the IDE command, the data location (a "Show in folder" button selects `app.db` in the file manager) and backup, export and import. Results of those actions show as a toast at the bottom right.
- Timesheet: a sidebar view with a per-month table per project, built from the author's git commits (stored in SQLite, migration 4); copies as tab-separated text.
- Settings live in `settings.txt` (key=value) in the data directory: last project and task, terminal visibility and width, terminal scrollback, IDE command, theme (dark, light, system), reduced transparency, reduced motion. Theme, transparency, motion and scrollback are changed in the Settings view or the command palette.
- Empty states have a short message and one primary action.

## Performance rules

Memory efficiency is a core goal:
- Load data lazily; do not load all tasks, notes or commits at startup.
- Page git history (for example 200 commits at a time).
- Avoid cloning large strings and buffers; prefer references, Arc, and Cow.
- Stream large files instead of reading them whole.
- Cap terminal scrollback (configurable, sensible default).
- Profile before optimizing. Add a note in the PR or commit when a change is performance-motivated.

## Coding conventions

- Run formatting and lint before finishing any change:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

- No unwrap or expect in non-test code unless the invariant is documented in a comment.
- Prefer small functions and explicit types at module boundaries.
- Public items get doc comments. Explain why, not what.
- Keep dependencies minimal. Justify any new crate (maintenance, size, license) in the commit message.
- Name things by domain (Project, Task, CommitLink), not by technical role.

## Commands

```bash
cargo build                         # debug build
cargo run -p app                    # run the app
cargo build --release               # optimized build
cargo test --workspace              # all tests
cargo test -p core                  # domain tests only
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Debug builds read `DEVSPACE_DATA_DIR`, `DEVSPACE_SIZE` (`1280x760`) and `DEVSPACE_VIEW` (`settings`, `history`, `run`, `timesheet`, `credentials`, `palette`, `terminal`, `task:<id>[:notes|attachments|commits]`, `note:<id>[:edit]`) so a screenshot run can start on a given screen against a copy of the data without synthetic input (see `qa.rs`). They compile to nothing in release. GPUI is very slow unoptimised, so judge typing and scrolling responsiveness on `cargo run --release -p app`, not on the debug build.

## Testing

- core and storage must have unit tests (use in-memory SQLite for storage tests).
- git crate tests create temporary repos with git2 and assert commit linking, branch views and patch-id matching, including the rebase and missing-commit case. Git client operations (status, hunk/line staging, commit, branch switch, stash, and CLI-backed fetch/pull/push/merge/rebase against a local bare remote) get the same temp-repo tests; tests skip CLI cases when `git` is not on PATH.
- Encryption tests: round trip, wrong key fails, nonce is never reused.
- UI is tested manually; keep logic out of views so it can be tested elsewhere.

## Cross-platform notes

- Target Windows, macOS and Linux. Check GPUI's current platform support for each before relying on a feature.
- Terminal: ConPTY on Windows (handled by portable-pty). Default shell differs per OS; make it configurable.
- IDE launching: run the configured command (for example code) with the project path. Keep the command configurable per user.
- Use std::path and dirs; never assume path separators.

## How Claude should work here

- Read the relevant crate and existing patterns before making changes.
- Make focused changes. Do not refactor unrelated code in the same change.
- Ask before adding a new dependency, changing the database schema, or changing the encryption scheme.
- If a GPUI API is uncertain, check the pinned version's source instead of guessing.
- After changes, run fmt, clippy and tests, and report any failures honestly.
- Never commit secrets, test credentials or personal paths.
- For any UI work, use the theme tokens; never hardcode colors, radii or shadows in a view.
- Check that every new screen is readable on light and dark backgrounds and works with transparency reduced.
-