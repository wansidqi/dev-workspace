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
      reader thread; kills the shell on drop) and `TerminalEmulator`
      (alacritty_terminal VT100 grid: styled snapshots, selection, scrollback
      cap, query replies), 12 unit tests; used by the terminal panel (see
      Developer tools)

## Projects

- [x] Create, rename, archive, switch (sidebar list, click to select)
- [x] Dashboard: name, creation date, task-count stat tiles
      (Todo/In Progress/Blocked/Done)
- [x] Dashboard: recent notes section (top 5 by `updated_at`, click to open
      in the note editor)
- [x] Link one or more git repos (list/link/unlink; type a path or use
      "Browse…" for the native folder picker — a subfolder resolves to its
      repo root; name defaults to the folder name)
- [x] Dashboard: recent commits section (merged across all linked repos, top 5 newest, repo-labeled)

## Tasks and subtasks

- [x] Unlimited nesting (depth-indented, parent/child via `parent_id`)
- [x] Status cycling (Todo -> In Progress -> Done; Blocked not reachable
      from the UI yet, only via direct `status` field)
- [x] Create, rename, delete (delete cascades to subtasks)
- [x] Priority: chip on the task detail page, click cycles
      Low -> Medium -> High -> Urgent (colour-coded)
- [x] Due date: click-to-edit on the task detail page (`YYYY-MM-DD`, empty
      clears, typos are rejected with a message); shows "Overdue" in red for
      unfinished tasks past their date (UTC day)
- [x] Tags (migration 3, approved): `task_tags(task_id, tag)` with a
      case-insensitive primary key and cascade on task delete. Add/remove on
      the task detail page (chips + "Add tag…", Enter adds; rules: trimmed,
      inner whitespace collapsed, max 32 chars, no commas or control
      characters, unique per task ignoring case), chips on task rows (first
      3 + "+N"), and a tag filter row on the project page ("All" + each tag
      with its task count; filtered tasks show flat since parents may be
      hidden). Rules live in `devspace-core::normalize_tag` and are enforced
      again in `TaskRepo::add_tag`.
- [x] Branch field: click-to-edit on the task detail page (blank clears);
      feeds the branch view and suggested commits
- [x] Task detail page (`task_detail.rs`, "Open" on a task row): breadcrumb
      of ancestors, status chip, tabs for **Notes**, **Attachments** and
      **Commits** with counts; task rows show "2 notes · 1 file · 3 commits"
- [x] Commits tab: the task's mapped commits (title/author/date read from
      git in the background), per-link status chip (click to cycle), unlink,
      "Map commits…" jumps to the commit browser; a commit git can no longer
      find (rebased/squashed or repo moved) is shown with a warning

## Notes

- [x] Create, rename, delete (project-level only)
- [x] Open and edit content (multiline editor, autosaves to the file)
- [x] Markdown stored as plain files under `WorkspacePaths::notes_dir`
- [x] Task-level notes: create, open (same editor), delete from the task
      detail page → Notes tab; works at any subtask depth. The project page
      lists only project-level notes.
- [ ] Kind selector — everything is created as `NoteKind::Note`; Script/SQL
      kinds exist in the domain type but aren't reachable from the UI
- [ ] Syntax highlighting
- [ ] Live markdown preview
- [ ] Search

## Attachments

- [x] Attach files to a task or subtask at any depth (task detail page →
      Attachments tab): "Add files…" opens the native multi-file picker.
      Per attach you choose **Copy into workspace** (streamed on a background
      task into `<project>/attachments/<id>-<sanitized name>`, never
      overwriting) or **Link by path** (only the path is stored).
- [x] Open an attachment with the OS default app; programs and scripts
      (`.exe`, `.bat`, `.ps1`, `.lnk`, `.jar`, extension-less, ...) are only
      revealed in the file manager, never launched (attachments are
      untrusted input). "Show in folder" and remove (✕) per file; removing a
      copied file deletes it from the workspace, a linked original is never
      touched.
- [x] Path safety: a copied attachment must canonicalize to inside the
      project's attachments folder (symlinks and tampered db rows are
      rejected for open and delete); deleting a task deletes its (and its
      subtasks') copied files and note files, not just the db rows.
- [ ] Project-level attachments (storage supports `task_id = NULL`, no UI)
- [ ] Built-in PDF viewer (PDFs open in the default app for now)
- [ ] Image preview, rename, drag-and-drop onto the page

## Credentials vault

- [ ] Not started: per-project encrypted storage
- [ ] AEAD encryption, OS keychain master key
- [ ] Reveal/copy with auto-hide and clipboard auto-clear
- [ ] Terminal env-var injection

## Git integration

- [x] `devspace-git`: git2 wrapper — `recent_commits`, `suggest_commits_for_task`,
      `merged_history`, `search_history`, `repo_root`
- [x] Repo linking on projects (list/link/unlink/browse — see Projects section)
- [x] Multi-repo history (`devspace_git::merged_history`): merges commits from
      all repos linked to a project, newest first, each row labeled with its
      repo, cursor-paged (cursor = time, repo id, hash, so same-second and
      same-prefix commits never repeat or vanish), unreadable repos reported
      rather than fatal. Each repo fetches one extra commit so a page ending
      exactly at the limit still offers the next page (regression-tested).
- [x] Commit history browser (`crates/app/src/commit_browser.rs`, opened from
      "Commit history" in the sidebar, replaces the main content): GitHub
      Desktop-style list, virtualized (`uniform_list`), pages of 200 loaded
      on a background task and prefetched near the end; live debounced search
      by title / author / hash prefix (`search_history`); repo filter chips;
      "Unassigned only" filter; "Select all". Open items: filter by branch and
      date range, keyboard navigation (arrows), a full diff/detail view.
- [x] Tag/map commits to tasks: click / Ctrl-click / Shift-click range
      selection, then pick any task from the project's tree (indented by
      depth) in the side panel, with a label (Linked / Needs migration /
      Cherry-picked / Deployed) and an optional note. Many-to-many, identified
      by (repo_id, hash), already-linked commits are skipped, errors (e.g. the
      cross-project rule) shown inline.
- [x] Per-link status and note: shown on the selected commit's "Mapped to"
      list; click the status chip to cycle it, ✕ to unlink. (Editing an
      existing note is not in the UI yet — note is set when mapping.)
- [x] Badges on commits: each row shows chips for up to 2 mapped tasks,
      coloured by status, plus "+N".
- [x] Project-scope rule: `core::check_same_project`, enforced in
      `CommitLinkRepo::insert` and by DB triggers (migration 2) so raw writes
      cannot cross projects; tested at 3 task depths and for cross-project
      rejection both ways
- [x] `commit_links` storage: `CommitLinkRepo` (insert, bulk
      `insert_skipping_existing`, get, list by task / commit / project,
      status+note update, delete, `count_for_repo`). Migration 2 adds
      UNIQUE(task_id, repo_id, hash) and the project-scope triggers.
- [x] Unassigned commits view (the "Unassigned only" filter in the browser)
- [x] Badges on a *task*: the project page rows show "N commits", and the
      task detail Commits tab lists them
- [x] Repo unlink handling when links exist: the ✕ on a linked repo warns
      "N commit mappings will be deleted with this repo" and needs a second
      click (with Cancel); repos with no mappings unlink immediately
- [x] Branch view: set a branch on a task and its Commits tab lists, per
      repo, the commits on that branch that are not in the target (target
      auto-detected: `origin/HEAD`, then `main`, `master`, `develop`; local
      branch first, then `origin/<branch>`), with a one-click "Map to task"
      and "Mapped ✓" for ones already linked. git2 only; unsafe names are
      refused. (`devspace_git::branch_only_commits`, `default_target_branch`)
- [x] Cherry-pick export: "Copy cherry-pick list" on the Commits tab copies
      per-repo shell text, oldest commit first, `git cherry-pick <hashes>`
      with a `# run inside: <path>` comment (no `cd`), missing commits as
      comments. Output is injection-safe (only comments and validated hex
      hashes). Pure logic in `devspace_core::cherry_pick_script`. Ordering is
      by commit time; same-second commits keep link order, not parent order.
- [x] Suggested commits: the Commits tab lists commits that mention `#<task
      id>` or the task's branch name (whole-token match, so `#4` never
      matches `#42`), newest first, scanning the newest 2000 commits per
      repo, with one-click mapping. (`devspace_git::suggested_for_task`;
      this also fixed `suggest_commits_for_task`, which used a naive
      substring match.) Message references work
      only if you write `#<id>` in the message; the id is shown next to the
      title on the task detail page.
- [ ] Missing-commit detection after rebase/squash: partly there — the task
      detail Commits tab warns for any mapped commit git cannot find
      (`devspace_git::lookup_commits` returns `None`), but nothing re-matches
      it by patch-id or marks the link `Missing` yet (`CommitInfo::patch_id`
      is currently the tree id, and links store none)

## Developer tools

- [x] Terminal: GPUI view (`crates/app/src/terminal_view.rs`) on the
      `devspace-terminal` backend — polls pty output on a 16ms background
      timer, answers ConPTY's cursor-position query, renders colours/bold/
      underline/inverse and a block cursor, resizes the pty to the panel's
      pixel size, wheel scrollback, mouse + double-click word selection,
      copy (Ctrl+C with selection, Ctrl+Shift+C, right-click), paste
      (Ctrl+V, Ctrl+Shift+V, Shift+Insert, bracketed-paste aware, ESC
      stripped), modifier-aware arrows (Ctrl+Left/Right word jumps),
      restart-after-exit on Enter.
- [x] Terminal: right-hand panel with tabs (`terminal_panel.rs`), one tab
      set per project; switching projects keeps every shell running,
      sidebar shows a `>_ N` badge per project, closing a tab kills its
      shell. Ctrl+` toggles the panel, Ctrl+Shift+` opens a new tab
      (same as VS Code).
- [x] Terminal: scrollback capped (2000 lines default, configurable in the
      backend; no settings UI yet)
- [ ] Terminal: tab rename, drag-to-resize panel width, split panes
- [ ] Terminal: scrollback setting in a settings UI
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
  `link_repo`, `unlink_repo`) calls `devspace_git::merged_history` over all
  linked repos (top 5 for the dashboard). Repos that fail to open land in
  `AppState::unreadable_repos` and show as a warning line instead of failing
  the whole refresh. It still runs on the UI thread — fine for 5 commits,
  but the full commit browser must page on a background task.
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
- Requirement (from user): commit history must be taggable/mappable to the
  project's tasks and subtasks at any nesting depth, strictly within the
  project, and a project can have multiple git repos. Delivered: the commit
  browser merges all repos' history; links key on (repo_id, hash) with the
  project-scope check in core, storage and DB triggers.
- Next most valuable slices: showing linked commits on task rows + the
  repo-unlink warning (both small, storage is ready), suggested commits and
  cherry-pick export, the command palette (unblocked by `TextInput`), a
  credentials vault (needs the keyring + AEAD decisions CLAUDE.md calls
  out), and due dates/tags/branch field on tasks.
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
- Migration 2 (`crates/storage/src/migrations.rs`) added
  `UNIQUE(task_id, repo_id, hash)` plus two triggers
  (`commit_links_same_project_insert/update`) so the project-scope rule holds
  even for writes that bypass `CommitLinkRepo`. Constraint failures are
  mapped back to `CommitLinkError::{AlreadyLinked, ProjectMismatch}` in
  `commit_links.rs`. Migration 1 was not touched.
- `CommitBrowser` patterns worth reusing: (1) the list is a `uniform_list`
  fed by `cx.processor(...)`, which gives the row builder `&mut Self` so rows
  are built only for the visible range without cloning the data set; the same
  callback triggers `load_more` near the end. (2) Live search observes the
  `TextInput` entity (it notifies on every edit; `TextInput` has no change
  callback), compares the text, and debounces 250 ms with an epoch counter.
  (3) Every reload bumps `generation` so a slow background page for an old
  query is dropped. (4) Selection is a set of `(RepoId, hash)` keys, not row
  indices, so it survives re-filtering.
- Bug found while building it: `delete_task` only removed direct children
  from the in-memory task list although the database cascades to every
  descendant, so deeper subtasks lingered in the UI (and in the mapping
  picker). It now reloads the task list and the commit links.
- Bug found while building it: `merged_history` reported "last page" when a
  page ended exactly at the limit. Each repo now fetches `limit + 1`.
- Folder picker: `Root::browse_repo` uses `cx.prompt_for_paths` (directories
  only) and resolves the chosen folder through `devspace_git::repo_root`
  (`Repository::discover`), so picking a subfolder links the repo root.
  A folder outside any repo is still linked; the dashboard and history views
  then warn "Couldn't read history for: <name>".
- Global shortcuts (Ctrl+`, Ctrl+Shift+`) are GPUI actions bound with no key
  context. Actions only reach elements on the focused element's ancestor
  path, so `Root` owns a `FocusHandle`, is focused at startup, and takes
  focus back when the terminal panel closes. Without that the shortcut
  silently did nothing until some input was clicked.
- Not unit-tested: the GPUI views (terminal panel/view, commit browser) —
  per CLAUDE.md they are tested manually, with the logic kept in
  `devspace-terminal`, `devspace-git`, `devspace-core` and `devspace-storage`.
- Task detail page (`crates/app/src/task_detail.rs`): `Root::open_task`
  stays set while a note is open, and the main-content precedence is
  history > note > task > project, so closing a note from the task page
  returns to the task page. Notes opened from the project page behave as
  before. Git details for the Commits tab are fetched in the background and
  re-fetched when the set of mapped commits changes (state observer).
- Attachments are a two-step operation in `AppState`:
  `prepare_attachment` (validate, allocate the id-prefixed stored name) →
  `AttachmentJob::run` (streams the copy; call it off the UI thread) →
  `finish_attachment` (insert the row, or delete the half-made copy if that
  fails). `AttachmentRepo::insert` checks the task belongs to the
  attachment's project in code; unlike commit links there is no DB trigger
  for it (that would need migration 3 — ask first).
- `is_launch_risky` / `sanitize_file_name` live in `devspace-core`
  (`attachment.rs`) so the "never launch executables" rule and the stored
  name rules are unit-tested without the UI. `external.rs` passes paths as
  discrete process arguments (`explorer.exe`, `open`, `xdg-open`) — never
  through a shell, so a hostile file name cannot inject a command.
- `AppState` now has unit tests (`state.rs`), using a self-cleaning scratch
  dir under the OS temp folder instead of adding the `tempfile` crate to the
  app crate. They cover the attachment copy/link/remove flows, the
  tampered-path containment check, task notes at depth, and file cleanup
  when a parent task is deleted.
- The cherry-pick export and the git branch/suggestion functions were written
  by two parallel subagents, split by file (`core/commit_link.rs` and
  `git/lib.rs`) so they could not collide; the UI wiring in `task_detail.rs`
  was done afterwards in the main session. Each was checked by re-running its
  tests and reading the security-relevant code (hash and ref-name validation,
  no shelling out outside tests) rather than trusting the hand-back.
- Task detail extras (`refresh_extras`) are computed in the background and
  keyed on (branch, linked repos, mapped commits); a stale result is dropped
  if the key changed while it ran, same generation idea as the commit browser.
- Migration 3 added `task_tags` (see Tasks). Tags load with their tasks:
  `TaskRepo::list_for_project` runs one extra query for the whole project
  (not one per task) and `get` loads a single task's tags. `TaskRepo::insert`
  saves a task's tags in the same transaction. `update` does not touch tags —
  use `add_tag`/`remove_tag`. The upgrade path is tested (a database built at
  migration 2 with existing data gets migration 3 applied cleanly).
