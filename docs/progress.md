# Devspace implementation progress

Tracks what's built against the CLAUDE.md spec, organized by feature area
(not by when it landed). Check items off as they're built; when a feature is
partially done, keep the parent unchecked and check off its sub-items so the
remaining gap is visible at a glance.

## Foundation

- [x] Cargo workspace: core, storage, git, terminal, runner, app crates
      (`cargo fmt`, `clippy -D warnings` and `cargo test --workspace` all
      pass as of the Timesheet commit; the app builds and launches)
- [x] `devspace-core`: Project, Task, Note, Attachment, CredentialMeta,
      CommitLink, RunCommand domain types + validation + unit tests
- [x] `devspace-storage`: SQLite migrations (WAL, foreign keys),
      `WorkspacePaths` for the on-disk layout
- [x] GPUI wired up and pinned to a Zed commit, glass theme tokens (dark = neutral black/gray surfaces with a blue accent; tag and repo chips get a stable per-name colour via `theme::label_color(&theme, ...)`, lightness follows the theme; whole UI in the monospace family, Cascadia Mono on Windows, set per-OS in `theme.rs`;
      light theme switchable at runtime, see UI polish)
- [x] `devspace-core`: Repo domain type (added alongside repo linking)
- [x] `devspace-runner`: std-only process runner (no dependencies) behind run commands
- [x] `devspace-terminal`: `PtySession` (portable-pty process + background
      reader thread; kills the shell on drop) and `TerminalEmulator`
      (alacritty_terminal VT100 grid: styled snapshots, selection, scrollback
      cap, query replies), 12 unit tests; used by the terminal panel (see
      Developer tools)

## Projects

- [x] Create, rename, archive, switch (sidebar list, click to select)
- [x] Project page header: name, creation date, repo count and a segmented status strip with counts
      (Todo/In Progress/Blocked/Done)
- [x] Dashboard: recent notes section (top 5 by `updated_at`, click to open
      in the note editor)
- [x] Link one or more git repos (list/link/unlink; type a path or use
      "Browse…" for the native folder picker — a subfolder resolves to its
      repo root; name defaults to the folder name)
- [x] Dashboard: recent commits section removed at the user's request (the history browser covers it); a warning still lists repos whose history could not be read

## Tasks and subtasks

- [x] Unlimited nesting (depth-indented, parent/child via `parent_id`)
- [x] Tree view on the project page: rounded L connectors from each parent's
      status icon to its subtasks, trunk continues past later siblings, row
      hairlines only between top-level groups. Line placement is pure logic
      (`tree_guides`, unit-tested) in `project_page.rs`; not shown under a tag
      filter (that list is flat).
- [x] Status cycling: Todo -> In Progress -> Done -> Blocked -> Todo (Blocked
      sits after Done so the common path stays two clicks). Click the status icon on a task row or the status pill on the task page; clicking the rest of a row opens the task
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

- [x] Project-level notes: create, rename, delete. (Task notes: create, open,
      delete from the task detail page — no rename yet.)
- [x] Open and edit content (multiline editor, autosaves to the file)
- [x] Markdown stored as plain files under `WorkspacePaths::notes_dir`
- [x] Task-level notes: create, open (same editor), delete from the task
      detail page → Notes tab; works at any subtask depth. The project page
      lists only project-level notes.
- [x] Every note is a markdown note: the "kind" chip was removed from the project page and the task Notes tab, and new notes are always plain notes. The `kind` column, `NoteKind` and `AppState::new_note_kind` remain (schema untouched); older Script/SQL notes keep their label and icon and render as markdown.
- [x] Syntax highlighting in the preview: fenced code blocks are coloured by a dependency-free tokenizer (`highlight.rs`: comments, strings, numbers, keywords, function calls; SQL gets a full keyword set, other languages a basic one, unknown languages only strings/numbers/comments). Unit-tested; not a parser. The editor itself is not highlighted.
- [x] Markdown preview: Edit / Split / Preview toggle, formatting toolbar (bold, italic, heading, code, table, lists, tasks, quote, link, rule), Ctrl+S saves and switches to Preview, double-clicking the rendered note switches to Edit; notes with content open in Preview, empty ones in Edit. The toolbar has a SQL button (inserts a fenced `sql` block). Code blocks render as cards with a header (language label) and a copy button that shows "Copied" for 1.6 s; long lines wrap (a mouse wheel cannot scroll sideways). The editor scrolls vertically (the frame scrolls, the padded text box inside grows) but does not yet follow the cursor and does not wrap long lines. `pulldown-cmark` (approved) parsed in `markdown.rs`, rendered by `markdown_view.rs`, page in `note_page.rs`, toolbar logic in `note_format.rs`; raw HTML shown as text, input capped at 512 KB. Notes are always `.md` files. Verified by tests and compilation only.
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
- [x] Project-level attachments: a "Files" section on the project page (`project_files.rs`) with the same copy-or-link choice, open (programs only revealed) and remove as the task tab; the attach loop is shared in `attach_flow.rs`. Tested in `AppState`; the section itself has not been clicked by hand.
- [ ] Built-in PDF viewer (PDFs open in the default app for now)
- [ ] Image preview, rename, drag-and-drop onto the page

## Credentials vault

- [x] Per-project encrypted storage: `credentials` table (migration 1, no schema change), `devspace-storage::CredentialRepo`, `AppState` helpers (`state/credentials.rs`). Metadata queries never read the ciphertext.
- [x] AEAD + OS keychain (approved crates `chacha20poly1305`, `keyring`, `zeroize`): XChaCha20-Poly1305 with a fresh random 24-byte nonce per secret; the credential and project ids are bound in as authenticated data so a ciphertext copied to another row will not open. The 32-byte master key is created on first use and kept in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service), never on disk; a damaged keychain entry is reported, never replaced. Secrets come back in `Zeroizing` and are wiped on drop. Limits: label 100 chars, secret 32 KiB. Tested: round trip, wrong key, tampering, moved ciphertext, nonce uniqueness over 2000 seals, no plaintext in the database, per-project scoping (plus an opt-in `--ignored keychain` test that passed on Windows).
- [x] Credentials view (sidebar "Credentials", `credentials_panel.rs`): add (label, kind chip, masked value), Reveal/Hide (auto-hides after 15 s, on project switch and on leaving the view), Copy (clipboard cleared after 30 s if it still holds the secret), Replace (rotate using the value box), delete with a second click. Opaque surface. The value box is a masked `TextInput` (one `*` per character, copy/cut disabled). Not part of project export/import; a workspace backup copies the database, and its ciphertext is useless without the keychain key. Compiles and launches; the screen has not been clicked by hand.
- [ ] Terminal env-var injection (optional, per CLAUDE.md)
- Known limits: GPUI text and the clipboard hold copies of a secret as ordinary strings that are not zeroized; a revealed value is drawn from such a copy.

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
      "Unassigned only" filter; "Select all". Open items: keyboard navigation
      (arrows), a full diff/detail view.
- [x] History filters by branch, author and date range:
      `devspace_git::filtered_history(repos, limit, after, &HistoryFilter)`
      (`search_history`/`merged_history` are thin wrappers). Branch reads that
      branch (local, then `origin/<name>`) per repo; repos without it are
      reported in `HistoryPage::missing_branch` (UI: "Branch x not in: ...");
      names are validated like `is_safe_ref_name`, git2 only. Author is a
      case-insensitive name substring; since/until are inclusive and the walk
      stops early once commits are older than `since`. `list_branches(repo)`
      (current, locals, remote-only) and `project_branches(repos)` feed the
      browser's filter bar: branch dropdown (searchable list, like GitHub Desktop), author box,
      Any time / Today / Last 7 / Last 30 days chips, From/To `YYYY-MM-DD`
      boxes with inline errors, "Clear filters". Day boundaries are UTC.
      UI verified by compilation only (not run); git side by unit tests.
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

## Git client (GitHub Desktop parity)

Decided 2026-10-20: grow the read-only git integration into a full git
client. Hybrid backend: git2 for reads and local index work, the system git
CLI (argument list, no shell, `GIT_TERMINAL_PROMPT=0`) for network and
history-rewriting operations — the same split GitHub Desktop uses (it bundles
git rather than using libgit2 for writes). See CLAUDE.md "Tech stack" and
"Security rules".

Multi-repo (applies to every phase): GitHub Desktop shows one repo at a
time; here each view covers all repos linked to the project, grouped and
labeled by repo, with a repo filter (like the history browser's chips) to
narrow to one. Git has no cross-repo commit, so multi-repo actions run once
per repo and report per-repo results; one repo failing never stops the rest.

Phase 1 — local working copy (git2 only). Built 2026-10-20 on
`feature/git-client`; the logic is unit tested, the screen was checked by
screenshot (see "Not verified by hand" below).
- [x] `devspace-git` working-copy client (`changes.rs`, `diff.rs`,
      `commit.rs`, `branches.rs`, `stash.rs`), 52 tests on temp repos built
      with git2 (no git executable needed):
  - [x] Status per repo: staged / unstaged / untracked / conflicted / renamed
        (git2's `path()` is the *old* name for a rename; the new one is read
        from the delta), branch, detached HEAD, unborn repo, upstream
        ahead/behind, merge/rebase-in-progress label
  - [x] File diff, unstaged (index → workdir, untracked as all-added) and
        staged (HEAD → index); binary, non-UTF-8 and over-long (20k lines)
        files are flagged
  - [x] Stage / unstage / discard by file, hunk and line: a patch with only
        the selected lines is built and applied to the index (or workdir for
        discard). CRLF and missing-final-newline files round-trip. The
        selection must come from the diff that was shown: if the file
        changed since, the call fails rather than staging the wrong lines
  - [x] Discard copies the files to `.git/devspace-discarded/<unix time>/`
        first (CLAUDE.md: prefer recoverable destructive actions); nothing
        prunes that folder yet
  - [x] Commit: summary + description + `Co-authored-by` trailers (validated
        `Name <email>`), amend, first commit in an empty repo; refuses with a
        readable message when nothing is staged, conflicts remain, or a
        merge/rebase is half done
  - [x] Branches: create (from the current commit or another branch), switch
        (also creates a tracking branch from `origin/<name>`), rename, delete
        (unmerged needs an explicit force); switching with changes either
        brings them (fails without touching anything if they'd be
        overwritten) or stashes them first
  - [x] Stash: save (with untracked files), list, apply, pop, drop
- [x] Changes view (`changes_view.rs`, `changes_view/`, pure logic in
      `changes_model.rs` with tests; sidebar "Changes", Ctrl+Shift+G, palette
      "Open git changes"):
  - [x] Multi-repo list: every linked repo's files in one virtualized list
        under collapsible repo headers; each repo's status loads on its own
        background task; repo filter chips narrow to one repo
  - [x] Repo header = the "repos overview": colour dot, name, changed-file
        count, current branch with ahead/behind, stash count, merge/rebase
        chip, a tri-state checkbox that stages or unstages the whole repo
        (not a separate overview screen)
  - [x] Diff viewer (virtualized): line numbers, +/- colouring, per-line
        checkboxes with Shift-click ranges, "Stage/Unstage hunk", a
        selection bar (Stage/Unstage/Discard lines), Unstaged/Staged switch
        for partly staged files
  - [x] Multi-repo commit: one message, one commit in each repo that has
        staged files; per-repo result lines; one repo failing doesn't stop
        the others. Co-authors box. Amend is single-repo (the filtered repo,
        or the only repo) and pre-fills the last message
  - [x] Branches popover for one repo or for all repos at once: switch,
        create, rename, delete, "bring / stash my changes" toggle; repos
        without the branch are reported as skipped, not failed
  - [x] Stashes are scoped to the checked-out branch (read from the stash
        message: `WIP on <b>:` / `On <b>:`); the header count and the popover
        show only those, with a link to reveal other branches' stashes.
        A direct "stash all changes" icon sits beside the repo's bin
  - [x] Stash popover per repo; discard confirmation bar (files, lines,
        branch delete, stash drop)
  - [x] Task tie-in: "Link commit to a task" picker (the project's task
        tree); commits made while a task is chosen are linked to it for every
        repo; if the task has a branch, "Use task branch in all repos"
        switches every repo to it, creating it where missing
  - [x] Refreshes after every action, on Refresh, and when the window regains
        focus (edits made in the IDE or terminal)
  - [x] Open a changed file in the IDE: double-click the row or the `<>` icon
        (uses the Settings IDE command; the path is checked to be inside the
        repo and passed as one argument, never through a shell)
  - [x] Popovers close on a click anywhere outside them (also the history
        browser's branch dropdown); the task picker is the same tree as the
        commit browser's, without status icons
- Not done / known gaps in Phase 1:
  - [ ] Syntax highlighting in the diff (colouring is +/- only)
  - [ ] Multi-line commit description (the input is single-line)
  - [ ] Git hooks do not run on commit (libgit2 doesn't run them; GitHub
        Desktop does). Phase 2 can route commits through the CLI if wanted
  - [ ] Keyboard navigation in the file list and diff
  - [ ] Commit-detail diff in the history browser (the diff code is reusable)
- Not verified by hand: clicks. The environment forbids synthetic input on
  the live desktop, so the click handlers are wired to tested git functions
  but not exercised in the running app. Screenshots checked: file list, diff,
  line selection, branch / stash / task popovers, discard confirm bar.
  `DEVSPACE_VIEW=changes[:select|branches|repo-branches|stash|tasks|confirm|lines]`
  with `DEVSPACE_QA_REPOS=<path>;<path>` reproduces them (debug builds).

Phase 2 — remotes (git CLI)
- [ ] "Fetch all" / "Pull all" / "Push all" across the project's repos,
      run in parallel with a small concurrency cap, per-repo progress and
      errors
- [ ] Resolve the git executable once; clear error when it is missing
- [ ] Fetch, pull (fast-forward / merge per config), push (set upstream on
      first push), ahead/behind counts, one "sync" button, periodic background
      fetch (off by default)
- [ ] Clone a repository and link it to the project

Phase 3 — integration
- [ ] Merge a branch into the current one; conflict list with "open in IDE",
      mark resolved, abort
- [ ] Revert a commit; undo last commit (soft reset, confirmation)

Phase 4 — history rewriting (git CLI)
- [ ] Interactive rebase: squash, reorder, edit message; drag-to-cherry-pick
      onto a branch; abort/continue on conflict
- [ ] After a rewrite, re-match commit links by patch-id (finishes the
      missing-commit item above)

Phase 5 — optional GitHub integration (off by default, network + account)
- [ ] Sign in with OAuth device flow; token in the OS keychain
- [ ] Pull request list for the current repo, create PR from a branch, CI
      check status on branches and commits

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
- [x] Terminal: scrollback capped (2000 lines default); the length for new terminals is set in the Settings view (100 to 100,000 lines, saved as `terminal_scrollback=`; a running terminal keeps the size it started with)
- [x] Terminal: double-click a tab to rename it (the old name is selected; empty names are ignored), drag the panel's left edge to resize it (320 to 1200 px, remembered as `terminal_width=`), and "Split" stacks a second shell under the first in the same tab (two panes at most; "Unsplit" closes the focused one). Checked in the running app. Not remembered across restarts: tab names and splits.
- [x] Per-project run commands + log view (`devspace-runner` crate,
      `run_panel.rs`; sidebar "Run commands"; add/edit/delete, Start/Stop,
      status chip, virtualised auto-scrolling log, Clear, Copy log; verified
      by compilation and unit tests only, not launched)
- [x] IDE launcher: "Open in IDE" on the project page (Settings view) runs
      the configurable command (default `code`, editable there, saved in
      settings) on the first linked repo or the project data folder. Spawned
      directly with the folder as one argument, never via a shell;
      quote-aware splitter, PATH/PATHEXT lookup (`code.cmd` works), and
      `.cmd` launchers refuse cmd metacharacters in the folder. Logic and
      tests in `ide.rs`.

## Search and navigation

- [x] Command palette (Ctrl+K / Ctrl+Shift+P): centered glass popover with a
      live-search `TextInput`; groups COMMANDS, PROJECTS, TASKS, NOTES, COMMITS
      (max ~50 rows); Up/Down/Tab, Enter, click, Esc, click-outside; restores
      the previous focus. Ranking is pure and tested (`palette_rank.rs`:
      exact > prefix > word-prefix > substring > fuzzy, stable, Unicode-safe)
- [x] Global search: task and note titles across all non-archived projects
      (`TaskRepo/NoteRepo::search_titles`, LIKE-escaped, bound params, no
      schema change; switches project first) and commits of the selected
      project via `search_history`; DB/git work is debounced on a background
      task
- [x] Global search of note content: the palette adds an "IN NOTES" group for notes whose file text contains every word (case-insensitive, at least 2 characters), with a one-line excerpt; notes already shown by title are skipped. `NoteRepo::search_contents` (`search_content.rs`) reads the files on the background search task, newest notes first, bounded: 400 notes, 512 KB per note, 32 MB per search, 12 results. Files are untrusted: only plain relative paths, regular files only (symlinks skipped). Unit-tested; not tried by hand.
- [x] Keyboard shortcuts: Ctrl+K / Ctrl+Shift+P (palette), Ctrl+` (terminal), Ctrl+Shift+` (new terminal), and Ctrl+Shift+H (commit history), Ctrl+Shift+R (run commands), Ctrl+Shift+M (timesheet), Ctrl+Shift+L (credentials) which open and again close the view, Ctrl+Shift+N (focus "New project"). Every shortcut is also a palette command that shows its hint; palette commands open a view and never close it. A test keeps the hints unique.
- [x] Restore last project, task page and terminal visibility on startup
      (`settings.rs`; stale ids fall back silently; a hidden terminal panel
      spawns nothing)

## Data and safety

- [x] Backup / export / import (`devspace-storage` `transfer.rs` and
      `import.rs`; buttons in the Settings view): workspace backup to
      `backups/<timestamp>/` (VACUUM INTO + project files), project export
      folder `<name>.devspace-export/` (`project.db` + `files/`, never
      credentials), import as a NEW project with all ids remapped in one
      transaction

## UI polish (glass design spec)

- [x] Window-level Mica/acrylic blur on Windows, custom draggable titlebar,
      glass surfaces with borders/shadows on sidebar + cards
- [x] Hover/selection transitions: `motion.rs` fades a surface in and out with a critically damped spring (about 150-200 ms, no overshoot; tested not to overshoot and to settle in 60-300 ms). Applied to the sidebar rows (project rows and the History, Run commands, Timesheet and Credentials rows via `Root::nav_row`), the terminal tabs, and the palette highlight. Other hovers (buttons, chips, task rows) are still instant. A "Motion: normal/reduced" setting (and palette command) turns the transitions off through GPUI's reduce-motion flag; GPUI does not read the OS flag, so this is a manual setting.
- [x] Light/dark theme: dark by default (the user's choice); "Theme: Dark / Light / Follow system" cycles at runtime in the Settings view or the palette and is saved (`theme=` in settings.txt). The theme lives in a GPUI global (`theme::ActiveTheme`) that every view re-reads in `render`, so a change restyles the whole app without a restart; with "Follow system" an OS switch is picked up too. Tag chips and terminal colours adapt to the light palette. Both palettes are tested for WCAG AA contrast on the solid surfaces. Checked in the running app (default dark, switching to light and back).
- [x] Reduced transparency: "Transparency: normal/reduced" (and a palette command) makes every surface opaque and switches the window backdrop from Mica to opaque; saved as `reduce_transparency=`. It does not yet follow the OS "transparency effects" setting.

## UI redesign (design system)

The whole interface was redesigned as one product (direction: graphite and phosphor).

- [x] Typography: Geist (UI) and Geist Mono (data, code, note source) bundled in `assets/fonts` and registered at startup (`theme::install_fonts`; a load failure falls back to the platform font). Terminal keeps a system monospace for glyph coverage. Six sizes and three weights via the `TypeScale` helpers; no raw sizes in views. This replaces the earlier all-monospace UI; `UI_FONT` in `theme.rs` is the one switch if that preference should come back.
- [x] Palette and shape: cool graphite surfaces, one lime accent (deep moss in light), muted status colours, new tokens (`surface_sunken`, `surface_hover`, `text_muted`, `border_strong`, `accent_fg`, `scrim`), radii 6/8/12, spacing to 32. Contrast tests cover every text and status token on both themes.
- [x] Icons: Lucide set embedded as SVG (`ui::Icon`), replacing text glyphs everywhere (old glyph call sites map through `Icon::from_glyph`).
- [x] Shared primitives in `ui.rs`: buttons (4 kinds), icon buttons, chips, filter chips, segmented control, switch, tabs, page header, empty state, notice, kbd.
- [x] Shell: title bar with a search field that opens the palette, 248px sidebar with project list and a Workspace group (Settings added, Ctrl+,), floating canvases for the view and the terminal, toast for action results, responsive width budget (terminal panel yields before the page drops under 520px; project rail stacks under 960px).
- [x] Project page recomposed (header, status strip, flat task list with floating row actions, side rail for notes/files/repos). Task page, commit history, timesheet, run commands, credentials, notes (editor, preview, toolbar), palette and terminal panel restyled with the same primitives. Workspace tools moved to a Settings view.
- [x] Window controls drawn as three status-light dots (see Decisions); notes preview redesigned as a page card with heading hairlines and code cards; word navigation in all text inputs.
- [x] Settings → Data: shows the path of `app.db` and a "Show in folder" button (`external::reveal`); notes and attachments sit in the folders beside it.
- [x] App icon: `assets/images/logo.png` is embedded as the exe's icon resource #1 by `crates/app/build.rs`, which writes the small binary `.res` itself (no resource-compiler crate); GPUI loads window and taskbar icons from that resource, and Explorer shows it for the .exe. MSVC targets only; one 256px PNG image (Windows scales it down). Replace the PNG and rebuild to change the icon; Explorer/taskbar may cache the old one until restart.
- [x] Debug-only QA hooks (`qa.rs`: `DEVSPACE_DATA_DIR`, `DEVSPACE_SIZE`, `DEVSPACE_VIEW` incl. `task:<id>:commits`, `note:<id>:edit`) for non-interactive screenshots.
- Performance: GPUI is very slow unoptimised. Typing in inputs lags in a debug build and is instant in `cargo run --release -p app`; judge responsiveness on release.
- Verified by screenshots at 900, 1300 and 1900 px wide in dark and light, with the terminal open and closed. Not exercised by hand: hover states, window dragging from the title bar around the search field, the light theme over a bright wallpaper with Mica.

## Timesheet

Ported from the standalone "git sheet" tool (which kept JSON files); data is
in SQLite (`app.db`) instead.

- [x] Migration 4 (approved): `timesheet_settings` (single row),
      `timesheet_reports` (month + remarks; a row exists once a month is
      saved), `timesheet_rows` (per project and month, unique on key,
      cascade on project delete) and `timesheet_commits` (commit snapshots
      per row, keyed by repo name + hash)
- [x] `devspace-core::timesheet`: ticket parsing (`7267:`, `#7267 -`,
      `[7267]`), grouping commits into one row per ticket, hours estimate,
      merge on re-sync (typed hours, edited titles and manual rows survive;
      estimated hours are recalculated), month helpers, tab-separated
      clipboard layout with `=SUM` formulas
- [x] `devspace-git::timesheet_commits`: a month of one author's commits
      across all local and remote branches, merges skipped, changed lines
      without lockfiles and generated output; the day is the author date in
      the author's own timezone
- [x] `devspace-storage::TimesheetRepo` and `AppState` helpers
      (`state/timesheet.rs`)
- [x] Sidebar "Timesheet" view (`timesheet_panel.rs`): month picker, one
      table per active project (reads that project's linked repos), click to
      edit title or hours, -/+ half-hour steppers, add or remove manual
      tasks, expandable commit list, remarks, Settings form (name, role,
      title, git authors, blank-row padding, estimate constants), "Sync from
      git" (background task) and "Copy all"; an unsaved month syncs on first
      open; every edit saves immediately
- Copy is plain text only (gpui's clipboard has no HTML), so values and SUM
  formulas paste into the sheet but fonts and borders do not. There is no
  Google Sheets push and no .xlsx export by decision.
- A repo that cannot be read leaves its project's table untouched on sync
  (with a message) rather than wiping saved rows.
- Timesheet tables are not part of project export/import or backup-by-project
  (workspace backup copies the whole database, so it is covered).
- The app launches with the Timesheet code in it; the screen itself still
  needs a hand test (month picker, "Sync from git", edits, "Copy all").

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
  called once at startup (already done in `main.rs`). Two options for forms:
  `masked` draws one `*` per character (byte offsets are converted to display
  offsets; copy, cut and the IME position hooks are disabled) and
  `keep_on_confirm` leaves the text in place after Enter (the default clears
  it), so a failed save does not wipe a typed secret.
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
  not on-screen x position. The editor is set in Geist Mono (monospace), so
  this is exact for ASCII; wide or combining characters can drift. Revisit
  with pixel-accurate hit-testing if it becomes one. Word movement
  (Ctrl+Left/Right, Ctrl+Shift+Left/Right, Ctrl+Backspace/Delete; Alt on
  macOS) lives in `text_nav.rs` and is shared with `TextInput` (masked inputs
  jump to the start or end of the whole value).
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
- Next slices (palette, global search, startup restore, backup/export,
  IDE launcher, run commands, markdown preview, Timesheet, note kinds,
  project files and the credentials vault are done):
  1. Hand-test the screens that only compile-checked so far (Timesheet,
     palette, run commands, backup/import, history filters, note page,
     credentials view).
  2. Terminal env-var injection from the vault (optional).
  3. Editor: soft-wrap long lines and scroll to the cursor.
  4. Built-in PDF viewer (needs `pdfium-render` approval).
  5. Patch-id re-matching of missing commits; light theme and reduced
     transparency; hover transitions.
- Layout gotchas found while testing in the running app (more of the same min-height/min-width `auto` default as the note below): the area holding sidebar, content and terminal panel needed `min_h(0)`, otherwise the whole window grew past its bottom edge and a second terminal pane was pushed off-screen; the project page column needed `min_w(0)` (it pushed the terminal panel off the right edge) and `overflow_y_scroll` (the project page could not scroll at all before). Full-content views are wrapped in `overflow_hidden` so a narrow window clips them instead of painting over the terminal panel. Two `flex_1` panes in a column did not split the height reliably; each split pane gets an explicit `relative(1/n)` height instead.
- `SettingsStore::update` now calls `cx.notify()` when something really changed. Without it nothing observing the store (the window restyling itself on a theme change) ever ran; it showed up only by clicking the theme button in the running app.
- `GlassTheme` gained `is_light`, `solid()` and `resolve(mode, os_is_dark, reduce_transparency)`. Views refresh `self.theme = theme::current(cx)` at the top of `render`; colours inside closures built during render are therefore always current.
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
- Window controls: with `appears_transparent: true` GPUI draws no caption buttons on Windows (an earlier note here claimed the OS paints them; it does not). The title bar draws its own three dots (minimise, maximise, close) marked with `window_control_area`. They must be siblings of the `Drag` region, not children: the hit test returns the first matching control area, so a drag region around them swallows their clicks. Terminal toggling stays on Ctrl+` and the palette.
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
- Run commands: `crates/runner` (`devspace-runner`) is std-only. It runs the
  user's saved command line via `cmd /S /C` (Windows) or `sh -c` and never
  interpolates anything else into it. stdout+stderr are split into lines by
  reader threads (lone `\r` ends a line, raw line bytes capped), sanitised
  (ANSI/OSC escapes, control chars and bidi overrides removed, 2,000-char
  cap) and kept in a 5,000-line ring buffer; a bounded channel slows a
  flooding process. `stop()` kills the tree (`taskkill /T /F`, or the Unix
  process group via `kill -KILL -PID`); dropping a `RunningCommand` kills it.
- `RunPanel` is owned by `Root` for the app's lifetime so commands survive
  navigation and project switches; it kills runs of archived projects (checked
  on each poll) and everything on app quit. Runs are not persisted across
  restarts, only the definitions are. `stop()` runs `taskkill`/`kill`
  synchronously on the UI thread (tens of ms); move it to a background task
  if that ever shows as jank.
- `AppState` run-command helpers live in `state/run_commands.rs` (a child
  module of `state`) with their own tests there.
- Settings file (`crates/app/src/settings.rs`): `key=value` lines
  (`last_project`, `last_task`, `terminal_open`, `ide_command`); unknown keys
  and malformed lines are ignored; written atomically (temp file + rename) 400
  ms after the last change by `SettingsStore`. `Root::sync_settings` runs per
  render but only schedules a write when a value differs. A write still
  pending when the app quits (under half a second) is lost.
- `AppState` id counters used to be raised from the *selected* project's rows
  only, but ids are global across projects; they are now also raised from
  global `MAX(id)` at startup and after an import (`refresh_id_counters`).
- Import safety (`devspace-storage::import`): the export is untrusted. Ids,
  file names and sizes are never reused (names are re-derived from new ids);
  note and copied-attachment paths must be plain relative names (no `..`,
  roots or drive prefixes) pointing at regular files (no symlinks) inside the
  export; copies are capped (2 GiB total, 1 GiB per file, enforced while
  reading); a schema newer than the app is refused; broken parent links and
  cycles are refused; bad tags/hashes/UNC links are skipped and reported.
  Imported run commands are data only, never executed by the import. Linked
  attachments keep their absolute path (a reference to a file on the
  exporter's machine).
- Backup/export/import use a second SQLite connection on a background thread
  (WAL, 10 s busy timeout), not the UI's connection.

## Pending decisions

- Dependencies awaiting approval:
  `pdfium-render` (PDF viewer). (`pulldown-cmark` is approved and in use; code highlighting is built in, no crate needed.)
- Git client Phase 5 (GitHub integration) would need an HTTP client crate
  (for example `ureq`, no tokio) — needs approval before that phase starts.
- History date filters use UTC day boundaries; a local-offset helper (or
  `chrono`, a new dependency) would make "Today" follow local time.
- Everything merged from the parallel agents (palette, run commands,
  settings/backup, history filters) was verified by compilation and unit
  tests; the screens have not been exercised by hand yet.
