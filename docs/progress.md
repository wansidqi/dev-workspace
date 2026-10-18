# Devspace implementation progress

Tracks what's built against the CLAUDE.md spec, organized by feature area
(not by when it landed). Check items off as they're built; when a feature is
partially done, keep the parent unchecked and check off its sub-items so the
remaining gap is visible at a glance.

## Foundation

- [x] Cargo workspace: core, storage, git, terminal, app crates
- [x] `devspace-core`: Project, Task, Note, Attachment, CredentialMeta,
      CommitLink, RunCommand domain types + validation + unit tests
- [x] `devspace-storage`: SQLite migrations (WAL, foreign keys),
      `WorkspacePaths` for the on-disk layout
- [x] GPUI wired up and pinned to a Zed commit, glass theme tokens (dark;
      light defined but unused)
- [x] `devspace-core`: Repo domain type (added alongside repo linking)
- [x] `devspace-terminal`: `PtySession` (portable-pty process + background
      reader thread) and `TerminalEmulator` (alacritty_terminal VT100 grid),
      unit-tested; not wired into any GPUI view yet (see Developer tools)

## Projects

- [x] Create, rename, archive, switch (sidebar list, click to select)
- [x] Dashboard: name, creation date, task-count stat tiles
      (Todo/In Progress/Blocked/Done)
- [x] Dashboard: recent notes section (top 5 by `updated_at`, click to open
      in the note editor)
- [x] Link one or more git repos (list/link/unlink; no repo-picker dialog —
      path is typed in, name defaults to the last path component)
- [x] Dashboard: recent commits section (first linked repo only, top 5 newest)

## Tasks and subtasks

- [x] Unlimited nesting (depth-indented, parent/child via `parent_id`)
- [x] Status cycling (Todo -> In Progress -> Done; Blocked not reachable
      from the UI yet, only via direct `status` field)
- [x] Create, rename, delete (delete cascades to subtasks)
- [ ] Priority (field exists in `devspace-core`, no UI)
- [ ] Due date
- [ ] Tags
- [ ] Branch field
- [ ] Per-task notes, attachments, linked commits

## Notes

- [x] Create, rename, delete (project-level only)
- [x] Open and edit content (multiline editor, autosaves to the file)
- [x] Markdown stored as plain files under `WorkspacePaths::notes_dir`
- [ ] Task-level notes (schema/storage support task_id already; no UI)
- [ ] Kind selector — everything is created as `NoteKind::Note`; Script/SQL
      kinds exist in the domain type but aren't reachable from the UI
- [ ] Syntax highlighting
- [ ] Live markdown preview
- [ ] Search

## Attachments

- [ ] Not started: PDFs, images, other files on project/task/subtask
- [ ] Built-in PDF viewer
- [ ] Copy-into-workspace vs link-by-path

## Credentials vault

- [ ] Not started: per-project encrypted storage
- [ ] AEAD encryption, OS keychain master key
- [ ] Reveal/copy with auto-hide and clipboard auto-clear
- [ ] Terminal env-var injection

## Git integration

- [x] `devspace-git`: git2 wrapper — `recent_commits`, `suggest_commits_for_task`
- [x] Repo linking on projects (list/link/unlink — see Projects section)
- [ ] Commit history browser UI (search/filter by branch/author/date) —
      dashboard's "recent commits" (top 5, first repo only) is as far as this
      has gone
- [ ] Link commits to tasks (single/multiple/range), per-link note and status
- [ ] Branch view (commits on task branch not in target)
- [ ] Unassigned commits view
- [ ] Cherry-pick export list
- [ ] Missing-commit detection after rebase/squash

## Developer tools

- [x] Terminal: GPUI view (`crates/app/src/terminal_view.rs`) wired to the
      `devspace-terminal` backend — spawns the platform shell in the
      selected project's first linked repo (or home dir), polls pty output
      on a 16ms background timer, forwards keystrokes (printable chars,
      Enter/Backspace/Tab/Escape, arrows, Home/End/Delete/Page, ctrl-letter)
      back to the shell. Toggled from a "Show/Hide terminal" row at the
      bottom of the project sidebar, rendered as a collapsible 260px bottom
      panel. Fixed 100x28 grid — not resized to the panel's actual pixel
      size yet.
- [ ] Terminal: tabs (one shared session today, not per-project)
- [ ] Terminal: scrollback cap setting (alacritty's default history applies)
- [ ] IDE launcher (configurable external command)
- [ ] Per-project run commands + log view

## Search and navigation

- [ ] Global search
- [ ] Command palette (unblocked — `TextInput` exists to build it on)
- [ ] Keyboard shortcuts / keybindings beyond the text-input ones
- [ ] Restore last project/task/layout on startup

## Data and safety

- [ ] Backup / export / import of a project

## UI polish (glass design spec)

- [x] Window-level Mica/acrylic blur on Windows, custom draggable titlebar,
      glass surfaces with borders/shadows on sidebar + cards
- [ ] Hover/selection transition animation (CLAUDE.md wants ~120-200ms
      ease-out; current hover states are instant)
- [ ] Light/dark toggle wired to OS setting (dark only so far)
- [ ] Reduced-transparency fallback setting (MicaBackdrop always on)

## Decisions / notes for future sessions

- GPUI pinned at rev `56934ec0560bce8929025497411de7f5322fb6ca`. Current API uses
  `gpui_platform::application()` as the entry point (split out of `gpui` itself).
  rustc updated to 1.99.1 to satisfy it.
- `url` crate pinned to 2.5.0 in Cargo.lock to keep `idna`/`icu_*` compatible —
  revisit if rustc gets bumped further and this pin is no longer needed.
- Windows path-safety check in `devspace-core::note::is_safe_relative_path` needs
  `Path::has_root()` in addition to `is_absolute()` — Windows treats `/etc/passwd`
  as non-absolute (no drive letter) but it's still rooted and must be rejected.
- `AppState` generates new project/task/note ids client-side (max(existing)+1)
  since there's no autoincrement wired through `core`'s newtype ids yet. Fine
  for a single-user local app; revisit if multi-process access is ever a thing.
- Windows glass backdrop: `WindowBackgroundAppearance::MicaBackdrop` calls into
  `DwmSetWindowAttribute` (DWMSBT_MAINWINDOW) under the hood — see
  `gpui_windows::window::set_background_appearance`. `Blurred` (older
  SetWindowCompositionAttribute acrylic path) is the fallback if Mica ever looks
  wrong on a given Windows build.
- If a running `devspace.exe` holds the build output locked (`Access is denied`
  on rebuild), `taskkill /F /IM devspace.exe` before retrying.
- `devspace-git` needed an explicit `build.rs` (`crates/git/build.rs`) linking
  `advapi32` on Windows: `libgit2-sys`'s own link directives don't pull it in on
  this rustc 1.99.1 + MSVC linker combo, which only surfaced when building
  `devspace-git`'s test binary standalone (`cargo test -p devspace-git`) — the
  `app` binary didn't show it because gpui transitively links advapi32 anyway.
  If a future rustc/MSVC bump makes this redundant, it's harmless to leave.
- `TextInput` (`crates/app/src/text_input.rs`) is a full single-line input:
  caret, mouse selection, clipboard, IME marked-text, Enter/Escape via
  `on_confirm`/`on_cancel` callbacks. Reusable — embed via `cx.new(|cx|
  TextInput::new(cx, placeholder, theme))`, set `on_confirm`, then
  `.child(entity.clone())` anywhere. Needs `text_input::init_key_bindings(cx)`
  called once at startup (already done in `main.rs`).
- `NoteEditor` (`crates/app/src/note_editor.rs`) is the multiline counterpart
  to `TextInput` — duplicated rather than generalized from it, because the
  single-line `TextInput` assumes one `ShapedLine` throughout (layout,
  hit-testing, cursor/selection painting) and threading a line index through
  all of that would have made both components harder to read. Content is one
  `String` split on `\n` into per-line `ShapedLine`s on each paint;
  cursor/selection track a byte offset into the whole string, converted to a
  (line, column) pair for painting and up/down movement. Autosaves via
  `on_change` on every keystroke — no explicit save action or dirty state.
  Enter inserts `\n` (no "confirm" concept); Escape closes. No IME
  marked-text handling tested yet (`replace_and_mark_text_in_range` is
  implemented for `EntityInputHandler` correctness but untested with an IME
  on this machine — unlike `TextInput`, which has been). Needs
  `note_editor::init_key_bindings(cx)` called once at startup (done in
  `main.rs`, alongside `text_input`'s).
  **Known limitation**: Up/Down move the cursor by column measured in bytes,
  not on-screen x position, so it drifts on lines whose rendered width
  differs if a proportional font is ever used (today's UI font is already
  proportional, but notes tend to be short/simple enough that this hasn't
  been an issue in manual testing). Revisit with pixel-accurate hit-testing
  if it becomes one.
- `GlassTheme` dropped its `shadow: Hsla` field — gpui's built-in `shadow_sm`/
  `shadow_md`/`shadow_lg` macros (tailwind-style, see `gpui_macros::styles`)
  cover this better than a single custom color token did; kept `warning` and
  `danger` and wired them to Blocked-status badges and destructive icon
  buttons respectively so clippy's dead-code lint stays honest signal, not
  noise to silence.
- Repo linking (`Repo` in `devspace-core::repo`, `RepoRepo` in
  `devspace-storage::repos`, `AppState::link_repo`/`unlink_repo`) stores just
  `path` + `name` — no validation that the path is actually a git repo at
  link time. `AppState::reload_recent_commits` (called from `select_project`,
  `link_repo`, `unlink_repo`) walks the **first** linked repo only and
  silently yields an empty list on any git error (not-a-repo, no commits,
  bad path) — same as the dashboard's "recent notes" section pattern, just
  without a stored "last refreshed" marker, since a fresh walk is cheap
  enough to redo on every mutation rather than cache more granularly. A
  project with several repos only ever sees the first one's history here —
  revisit (merge + sort by time across repos) if that turns out to matter in
  practice.
- `devspace-terminal` (`crates/terminal/src/lib.rs`) splits into two pieces
  that can be tested independently: `PtySession` (owns the `portable-pty`
  master/child, writes synchronously, reads via a background thread into an
  `mpsc` channel so nothing blocks on pty I/O) and `TerminalEmulator` (wraps
  `alacritty_terminal::Term`, fed raw bytes via `vte::ansi::Processor`,
  independent of where those bytes came from — the pty_session tests spawn
  a real shell; the emulator tests don't). `Term` needs a type implementing
  `alacritty_terminal::grid::Dimensions`; `TerminalSize { cols, rows }` is
  that type and also the pty's kernel-side winsize, so one struct serves
  both call sites. `EventListener` is implemented via an `mpsc::Sender`
  (`EventProxy`) rather than `VoidListener`, so title/bell/exit events are
  available on `TerminalEmulator::events` whenever the GPUI view gets built.
  **Windows ConPTY gotcha**: on spawn, conhost emits mode-setting escapes
  and a cursor-position query (`ESC [ 6 n`) before it'll process further
  input; a real terminal emulator answers it. The pty_session test does
  this manually (see `pty_session_runs_a_command_and_produces_output`) —
  the eventual GPUI terminal view should handle `TerminalEvent`/CPR the
  same way, or input will appear to hang on Windows.
- Next most valuable slice: the command palette (unblocked by `TextInput`),
  linking commits to tasks (the next git-integration step, now that repos
  are linkable — needs a commit-picker UI reading
  `recent_commits`/`suggest_commits_for_task` and writing `CommitLink`
  rows), or a credentials vault (nothing started yet, needs the keyring +
  AEAD decisions CLAUDE.md calls out). Due dates/tags/branch field on tasks
  are the other near-term item.
- GPUI flexbox gotcha hit wiring the terminal panel: a flex child with
  `.flex_1()` but no `.min_h(px(0.0))` won't shrink below its content's
  natural height, so a fixed-size sibling (the terminal panel) after it
  gets pushed outside the window instead of the row shrinking to make
  room — same `min-height: auto` default as CSS flexbox. Watch for this
  anywhere else a flex row sits above/beside a fixed-size panel.
- `TerminalView` (`crates/app/src/terminal_view.rs`) polls
  `PtySession::try_recv_output` on a `cx.spawn` loop using
  `cx.background_executor().timer(16ms)`, rather than reacting to an event —
  simpler than wiring a callback through `PtySession`'s reader thread, and
  16ms is cheap enough against a non-blocking `try_recv`. Key handling is a
  hand-rolled subset of xterm's escape sequences (see the function doc
  comment in that file) — covers normal shell use, not the full modifier
  matrix zed's `terminal::mappings::keys` implements; expand there if a
  workflow needs e.g. shift-arrow or alt-modified keys.
- The terminal toggle button almost went in the custom titlebar's top-right
  corner — don't do that. On Windows, the OS still paints native
  minimize/maximize/close caption buttons over that corner even with
  `appears_transparent: true` and no native titlebar content, silently
  eating anything GPUI draws underneath. Interactive controls belong
  elsewhere in the chrome (this one lives in the sidebar).
