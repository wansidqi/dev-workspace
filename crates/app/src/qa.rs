//! Debug-build conveniences for visual QA. They let a screenshot run start on
//! a chosen view, window size and data directory without synthetic input and
//! without touching the user's real workspace. Release builds compile these to
//! nothing, so none of it ships.
//!
//! Environment variables (debug builds only):
//! - `DEVSPACE_DATA_DIR`: use this folder instead of the OS data directory
//! - `DEVSPACE_SIZE`: window size as `WIDTHxHEIGHT`
//! - `DEVSPACE_VIEW`: `settings`, `history`, `run`, `timesheet`, `credentials`,
//!   `palette`, `terminal`, `task:<id>[:notes|attachments|commits]` or `note:<id>`

use std::path::PathBuf;

use gpui::{Context, Window};

use crate::Root;

/// Workspace folder override.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        std::env::var_os("DEVSPACE_DATA_DIR").map(PathBuf::from)
    } else {
        None
    }
}

/// Window size override, in logical pixels.
pub fn window_size() -> Option<(f32, f32)> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let raw = std::env::var("DEVSPACE_SIZE").ok()?;
    let (w, h) = raw.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Opens the view named by `DEVSPACE_VIEW`, if set.
pub fn open_view(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    if !cfg!(debug_assertions) {
        return;
    }
    let Ok(view) = std::env::var("DEVSPACE_VIEW") else {
        return;
    };
    match view.as_str() {
        "settings" => root.toggle_settings_view(cx),
        "history" => root.toggle_history(cx),
        "run" => root.toggle_run_view(cx),
        "timesheet" => root.toggle_timesheet_view(cx),
        "credentials" => root.toggle_credentials_view(cx),
        "palette" => root.toggle_palette(window, cx),
        "terminal" => root.toggle_terminal(window, cx),
        other => {
            if let Some(rest) = other.strip_prefix("task:") {
                let (id, tab) = rest.split_once(':').unwrap_or((rest, ""));
                if let Ok(id) = id.parse() {
                    root.open_task_detail(devspace_core::TaskId(id), cx);
                    if let Some(detail) = root.open_task.clone() {
                        detail.update(cx, |detail, cx| detail.show_tab(tab, cx));
                    }
                }
            } else if let Some(rest) = other.strip_prefix("note:") {
                let (id, mode) = rest.split_once(':').unwrap_or((rest, ""));
                let Ok(id) = id.parse() else { return };
                let id = devspace_core::NoteId(id);
                let title = root
                    .state
                    .read(cx)
                    .notes
                    .iter()
                    .find(|n| n.id == id)
                    .map(|n| n.title.clone())
                    .unwrap_or_default();
                root.open_note(id, title.into(), cx);
                if mode == "edit" {
                    root.set_note_mode(crate::note_page::NoteMode::Edit, window, cx);
                }
            }
        }
    }
}
