//! Per-user settings, stored as a tiny `key=value` text file at
//! `<data_dir>/devspace/settings.txt`. Hand-rolled on purpose: four fields
//! don't justify a serde/toml dependency (CLAUDE.md: keep dependencies
//! minimal), and a plain file stays human-editable and portable.
//!
//! The parse/serialize functions are pure and never panic, whatever the file
//! contains; [`SettingsStore`] adds debounced, atomic writes on top.

use std::path::{Path, PathBuf};
use std::time::Duration;

use devspace_core::TaskId;
use gpui::{Context, Task};

use crate::state::AppState;
use crate::theme::ThemeMode;

/// Settings files bigger than this are ignored (corrupt or not ours), so a
/// bad file can't make startup read unbounded data.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// How long to wait after the last change before writing, so rapid changes
/// (switching projects, toggling panels) coalesce into one write.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);
/// The IDE command used until the user changes it.
pub const DEFAULT_IDE_COMMAND: &str = "code";

/// What the app remembers between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Id of the project that was selected.
    pub last_project: Option<i64>,
    /// Id of the task whose detail page was open.
    pub last_task: Option<i64>,
    /// Whether the terminal panel was visible.
    pub terminal_open: bool,
    /// Command (program plus optional fixed arguments) used by "Open in IDE".
    pub ide_command: String,
    /// Width of the terminal panel, in pixels (set by dragging its edge).
    pub terminal_width: u32,
    /// Scrollback lines kept by each new terminal tab.
    pub terminal_scrollback: u32,
    /// Follow the OS light/dark setting, or force one.
    pub theme_mode: ThemeMode,
    /// Draw solid surfaces instead of translucent ones.
    pub reduce_transparency: bool,
    /// Skip transitions: hover and selection changes are instant.
    pub reduce_motion: bool,
}

/// Terminal panel width limits and default, in pixels.
pub const MIN_TERMINAL_WIDTH: u32 = 320;
pub const MAX_TERMINAL_WIDTH: u32 = 1200;
pub const DEFAULT_TERMINAL_WIDTH: u32 = 520;
/// Scrollback limits and default, in lines. The cap keeps one terminal's
/// history from growing without bound.
pub const MIN_TERMINAL_SCROLLBACK: u32 = 100;
pub const MAX_TERMINAL_SCROLLBACK: u32 = 100_000;
pub const DEFAULT_TERMINAL_SCROLLBACK: u32 = 2000;

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_project: None,
            last_task: None,
            terminal_open: false,
            ide_command: DEFAULT_IDE_COMMAND.to_string(),
            terminal_width: DEFAULT_TERMINAL_WIDTH,
            terminal_scrollback: DEFAULT_TERMINAL_SCROLLBACK,
            theme_mode: ThemeMode::Dark,
            reduce_transparency: false,
            reduce_motion: false,
        }
    }
}

impl Settings {
    /// Parses settings text. Lenient by design: unknown keys are ignored
    /// (so an older build can read a newer file), malformed lines and
    /// unusable values are skipped, and anything missing keeps its default.
    /// Splits on the FIRST `=`, so values may themselves contain `=`.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let line = line.trim_start_matches('\u{feff}');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "last_project" => settings.last_project = parse_id(value),
                "last_task" => settings.last_task = parse_id(value),
                "terminal_open" => settings.terminal_open = matches!(value, "true" | "1"),
                // A blank command keeps the default rather than breaking
                // "Open in IDE".
                "ide_command" if !value.is_empty() => settings.ide_command = value.to_string(),
                // Numbers are clamped into their range; unparsable ones
                // keep the default.
                "terminal_width" => {
                    if let Ok(n) = value.parse::<u32>() {
                        settings.terminal_width = n.clamp(MIN_TERMINAL_WIDTH, MAX_TERMINAL_WIDTH);
                    }
                }
                "terminal_scrollback" => {
                    if let Ok(n) = value.parse::<u32>() {
                        settings.terminal_scrollback =
                            n.clamp(MIN_TERMINAL_SCROLLBACK, MAX_TERMINAL_SCROLLBACK);
                    }
                }
                "theme" => {
                    if let Some(mode) = ThemeMode::parse(value) {
                        settings.theme_mode = mode;
                    }
                }
                "reduce_transparency" => {
                    settings.reduce_transparency = matches!(value, "true" | "1");
                }
                "reduce_motion" => {
                    settings.reduce_motion = matches!(value, "true" | "1");
                }
                _ => {}
            }
        }
        settings
    }

    /// Serializes to the text [`Self::parse`] reads back. Line breaks in
    /// the IDE command are flattened so a value can never spill onto (and
    /// forge) another line.
    pub fn serialize(&self) -> String {
        let mut out = String::from("# Devspace settings (key=value, one per line)\n");
        if let Some(id) = self.last_project {
            out.push_str(&format!("last_project={id}\n"));
        }
        if let Some(id) = self.last_task {
            out.push_str(&format!("last_task={id}\n"));
        }
        out.push_str(&format!("terminal_open={}\n", self.terminal_open));
        out.push_str(&format!("terminal_width={}\n", self.terminal_width));
        out.push_str(&format!(
            "terminal_scrollback={}\n",
            self.terminal_scrollback
        ));
        out.push_str(&format!("theme={}\n", self.theme_mode.as_str()));
        out.push_str(&format!(
            "reduce_transparency={}\n",
            self.reduce_transparency
        ));
        out.push_str(&format!("reduce_motion={}\n", self.reduce_motion));
        let command: String = self
            .ide_command
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        out.push_str(&format!("ide_command={}\n", command.trim()));
        out
    }
}

/// Reads what a user typed into the scrollback box: a whole number of lines
/// inside [`MIN_TERMINAL_SCROLLBACK`]..=[`MAX_TERMINAL_SCROLLBACK`]. Anything
/// else is an error message for the user, not silently clamped.
pub fn parse_scrollback(text: &str) -> Result<u32, String> {
    let lines: u32 = text
        .trim()
        .replace([',', '_'], "")
        .parse()
        .map_err(|_| "Enter a whole number of lines, e.g. 5000".to_string())?;
    if !(MIN_TERMINAL_SCROLLBACK..=MAX_TERMINAL_SCROLLBACK).contains(&lines) {
        return Err(format!(
            "Scrollback must be between {MIN_TERMINAL_SCROLLBACK} and {MAX_TERMINAL_SCROLLBACK} lines"
        ));
    }
    Ok(lines)
}

/// Only positive ids are real (ids start at 1).
fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}

/// Where the settings file lives inside the workspace folder.
pub fn settings_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join("settings.txt")
}

/// Reads the settings file. A missing, unreadable, oversized or garbled
/// file yields defaults; it never fails.
pub fn load(path: &Path) -> Settings {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_FILE_BYTES => {}
        _ => return Settings::default(),
    }
    match std::fs::read(path) {
        Ok(bytes) => Settings::parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Settings::default(),
    }
}

/// Writes the settings atomically: a temp file in the same folder, then a
/// rename over the real one, so a crash mid-write can't leave a torn file.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, settings.serialize())?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Applies saved settings to a freshly loaded [`AppState`]: reselects the
/// remembered project if it still exists and is not archived, and returns
/// the remembered task if it still exists in that project. Stale ids fall
/// back to the defaults silently.
pub fn restore_selection(state: &mut AppState, settings: &Settings) -> Option<TaskId> {
    let project = settings.last_project?;
    let exists = state.projects.iter().any(|p| p.id.0 == project);
    if !exists {
        return None;
    }
    if state.selected_project.map(|p| p.0) != Some(project) {
        state.select_project(devspace_core::ProjectId(project));
    }
    let task = settings.last_task?;
    state
        .tasks
        .iter()
        .any(|t| t.id.0 == task)
        .then_some(TaskId(task))
}

/// Holds the live settings and persists them. Lives in a GPUI entity so any
/// view can read or change a setting; writes happen on a background task
/// after a short quiet period and only when something actually changed.
pub struct SettingsStore {
    settings: Settings,
    path: PathBuf,
    /// The scheduled write; replacing it cancels the previous one.
    pending: Option<Task<()>>,
}

impl SettingsStore {
    /// Loads the settings from `path` (defaults if absent).
    pub fn load(path: PathBuf) -> Self {
        Self {
            settings: load(&path),
            path,
            pending: None,
        }
    }

    /// The current settings.
    pub fn get(&self) -> &Settings {
        &self.settings
    }

    /// Changes settings via `change`; if anything differs, schedules a
    /// debounced save. Cheap when nothing changed, so it is safe to call
    /// from a render pass.
    pub fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        if self.settings != before {
            // Observers (the window restyles itself on theme changes) hear
            // about real changes only, so a no-op update stays quiet.
            cx.notify();
            self.schedule_save(cx);
        }
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok(snapshot) = this.update(cx, |store, _| store.settings.clone()) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { save(&path, &snapshot) })
                .await;
            if let Err(e) = result {
                tracing::warn!("could not save settings: {e}");
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let settings = Settings {
            last_project: Some(7),
            last_task: Some(42),
            terminal_open: true,
            ide_command: "code --reuse-window".to_string(),
            terminal_width: 640,
            terminal_scrollback: 5000,
            theme_mode: ThemeMode::Light,
            reduce_transparency: true,
            reduce_motion: true,
        };
        assert_eq!(Settings::parse(&settings.serialize()), settings);
        assert_eq!(
            Settings::parse(&Settings::default().serialize()),
            Settings::default()
        );
    }

    #[test]
    fn unknown_keys_and_malformed_lines_are_ignored() {
        let text = "future_setting=on\n\
                    garbage line without equals\n\
                    =novalue\n\
                    last_project=3\n\
                    # a comment\n\
                    last_task=not a number\n\
                    terminal_open=true\n";
        let s = Settings::parse(text);
        assert_eq!(s.last_project, Some(3));
        assert_eq!(s.last_task, None);
        assert!(s.terminal_open);
        assert_eq!(s.ide_command, DEFAULT_IDE_COMMAND);
    }

    #[test]
    fn values_may_contain_equals_and_unicode() {
        let s = Settings::parse("ide_command=my-ide --profile=dév ✓ --x=\"a=b\"\n");
        assert_eq!(s.ide_command, "my-ide --profile=dév ✓ --x=\"a=b\"");
        assert_eq!(Settings::parse(&s.serialize()), s);
    }

    #[test]
    fn ids_must_be_positive_and_later_lines_win() {
        let s = Settings::parse("last_project=0\nlast_task=-4\n");
        assert_eq!((s.last_project, s.last_task), (None, None));
        let s = Settings::parse("last_project=1\nlast_project=2\n");
        assert_eq!(s.last_project, Some(2));
    }

    #[test]
    fn crlf_bom_and_blank_values_are_tolerated() {
        let s = Settings::parse("\u{feff}last_project=5\r\nterminal_open=1\r\nide_command=\r\n");
        assert_eq!(s.last_project, Some(5));
        assert!(s.terminal_open);
        assert_eq!(
            s.ide_command, DEFAULT_IDE_COMMAND,
            "blank keeps the default"
        );
    }

    #[test]
    fn dark_is_the_default_theme_and_a_missing_key_keeps_it() {
        assert_eq!(Settings::default().theme_mode, ThemeMode::Dark);
        assert_eq!(
            Settings::parse("last_project=1\n").theme_mode,
            ThemeMode::Dark
        );
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
    }

    #[test]
    fn numbers_are_clamped_and_bad_values_keep_the_defaults() {
        let s = Settings::parse(
            "terminal_width=99999\nterminal_scrollback=1\ntheme=sepia\nreduce_transparency=maybe\n",
        );
        assert_eq!(s.terminal_width, MAX_TERMINAL_WIDTH);
        assert_eq!(s.terminal_scrollback, MIN_TERMINAL_SCROLLBACK);
        assert_eq!(s.theme_mode, ThemeMode::Dark);
        assert!(!s.reduce_transparency);

        let s = Settings::parse("terminal_width=10\nterminal_scrollback=lots\ntheme=LIGHT\n");
        assert_eq!(s.terminal_width, MIN_TERMINAL_WIDTH);
        assert_eq!(s.terminal_scrollback, DEFAULT_TERMINAL_SCROLLBACK);
        assert_eq!(s.theme_mode, ThemeMode::Light);

        let s = Settings::parse("terminal_width=-5\nterminal_scrollback=100000000\n");
        assert_eq!(s.terminal_width, DEFAULT_TERMINAL_WIDTH);
        assert_eq!(s.terminal_scrollback, MAX_TERMINAL_SCROLLBACK);
    }

    #[test]
    fn scrollback_input_accepts_numbers_in_range_only() {
        assert_eq!(parse_scrollback(" 5000 "), Ok(5000));
        assert_eq!(parse_scrollback("10,000"), Ok(10_000));
        assert_eq!(parse_scrollback("100"), Ok(100));
        assert_eq!(parse_scrollback("100000"), Ok(100_000));
        for bad in ["", "abc", "-5", "12.5", "99", "100001", "99999999999999"] {
            assert!(parse_scrollback(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_command_cannot_inject_another_line() {
        let s = Settings {
            ide_command: "code\nlast_project=99".to_string(),
            ..Settings::default()
        };
        assert_eq!(Settings::parse(&s.serialize()).last_project, None);
    }

    #[test]
    fn garbage_never_panics() {
        for text in [
            "",
            "\0\0\0",
            "=",
            "====",
            "last_project=99999999999999999999999999",
            "\u{1F600}=\u{1F600}",
            &"x=".repeat(10_000),
        ] {
            let _ = Settings::parse(text);
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("devspace-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_or_binary_file_gives_defaults() {
        let dir = scratch("load");
        assert_eq!(load(&dir.join("settings.txt")), Settings::default());
        std::fs::write(dir.join("settings.txt"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        assert_eq!(load(&dir.join("settings.txt")), Settings::default());
        std::fs::write(dir.join("big.txt"), vec![b'a'; 200_000]).unwrap();
        assert_eq!(load(&dir.join("big.txt")), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_is_atomic_and_overwrites() {
        let dir = scratch("save");
        let path = settings_path(&dir);
        let mut s = Settings {
            last_project: Some(1),
            ..Settings::default()
        };
        save(&path, &s).unwrap();
        s.last_project = Some(2);
        save(&path, &s).unwrap();
        assert_eq!(load(&path).last_project, Some(2));
        assert!(
            !path.with_extension("txt.tmp").exists(),
            "temp file is gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn state_with_projects() -> (AppState, PathBuf) {
        let dir = scratch("state");
        let mut conn = devspace_storage::open_in_memory().unwrap();
        devspace_storage::run_migrations(&mut conn).unwrap();
        let paths = devspace_storage::WorkspacePaths::new(dir.clone());
        let mut state = AppState::new(conn, paths);
        state.create_project("First");
        state.create_project("Second");
        state.create_task("only task");
        (state, dir)
    }

    #[test]
    fn restore_selects_the_saved_project_and_task() {
        let (mut state, dir) = state_with_projects();
        let second = state.selected_project.unwrap().0;
        let task = state.tasks[0].id.0;
        let first = state
            .projects
            .iter()
            .find(|p| p.name == "First")
            .unwrap()
            .id
            .0;
        state.select_project(devspace_core::ProjectId(first));

        let settings = Settings {
            last_project: Some(second),
            last_task: Some(task),
            ..Settings::default()
        };
        assert_eq!(restore_selection(&mut state, &settings), Some(TaskId(task)));
        assert_eq!(state.selected_project.map(|p| p.0), Some(second));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_ids_degrade_to_defaults() {
        let (mut state, dir) = state_with_projects();
        let current = state.selected_project;
        let missing_project = Settings {
            last_project: Some(9999),
            last_task: Some(1),
            ..Settings::default()
        };
        assert_eq!(restore_selection(&mut state, &missing_project), None);
        assert_eq!(state.selected_project, current);

        let missing_task = Settings {
            last_project: current.map(|p| p.0),
            last_task: Some(9999),
            ..Settings::default()
        };
        assert_eq!(restore_selection(&mut state, &missing_task), None);
        assert_eq!(state.selected_project, current);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archived_projects_are_not_restored() {
        let (mut state, dir) = state_with_projects();
        let current = state.selected_project.unwrap();
        let other = state.projects.iter().find(|p| p.id != current).unwrap().id;
        state.archive_project(other);
        let settings = Settings {
            last_project: Some(other.0),
            ..Settings::default()
        };
        assert_eq!(restore_selection(&mut state, &settings), None);
        assert_ne!(state.selected_project, Some(other));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
