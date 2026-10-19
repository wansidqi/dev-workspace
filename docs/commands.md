# Commands

Run these from the repository root (`dev-workspace/`). The package is named `app`; the executable it produces is `devspace`.

## Run locally (development)

```
cargo run -p app                  # debug build, starts the app
cargo build                       # debug build only -> target\debug\devspace.exe
```

GPUI is very slow without optimisations, so typing and scrolling feel sluggish in a debug build. To judge responsiveness, run the release build instead:

```
cargo run --release -p app
```

Debug builds also read these environment variables (they compile to nothing in release), so a screenshot or QA run can start on a given screen against a copy of the data:

| Variable | Example | Effect |
|---|---|---|
| `DEVSPACE_DATA_DIR` | `D:\tmp\devspace-qa` | Use this folder instead of `%APPDATA%\devspace`. |
| `DEVSPACE_SIZE` | `1280x760` | Window size. |
| `DEVSPACE_VIEW` | `timesheet` | Open on a view: `settings`, `history`, `run`, `timesheet`, `credentials`, `palette`, `terminal`, `task:<id>[:notes\|attachments\|commits]`, `note:<id>[:edit]`. |

PowerShell example:

```
$env:DEVSPACE_DATA_DIR = "D:\tmp\devspace-qa"; $env:DEVSPACE_VIEW = "timesheet"; cargo run -p app
```

## Check the code

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # all tests
cargo test -p devspace-core       # one crate (core's package name is devspace-core)
cargo test settings               # tests whose name matches "settings"
```

Run `fmt`, `clippy` and `test` before every commit.

## Build for production

```
cargo build --release
```

The optimised executable is `target\release\devspace.exe`. It is a single file: the fonts and icons are embedded, and the Windows application icon is added by `crates/app/build.rs`, so you can copy the `.exe` anywhere and run it.

Production notes:

- A release build is slower to compile than a debug build (a few minutes the first time), then incremental.
- The QA environment variables above are ignored in release; the app always uses the OS data folder.
- Stop a running `devspace.exe` before rebuilding. Windows will not let cargo overwrite the executable while it runs (`Access is denied (os error 5)`).

## Where the data lives

| What | Location |
|---|---|
| Database | `%APPDATA%\devspace\app.db` |
| Notes and attachments | `%APPDATA%\devspace\projects\<project id>\` |
| Settings | `%APPDATA%\devspace\settings.txt` |
| Backups | `%APPDATA%\devspace\backups\` |

The release and debug builds share this folder, so a production run uses the same data you see in development. Back up `app.db` (Settings, Backup) before testing anything destructive.
