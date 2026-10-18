//! GPUI view for the integrated terminal (CLAUDE.md "Developer tools").
//! The pty + VT100 backend lives in `devspace-terminal`; this entity polls
//! pty output on a background timer so the UI thread never blocks on pty
//! I/O (CLAUDE.md GPUI guidelines), and forwards key presses back to the
//! shell.

use std::path::PathBuf;
use std::time::Duration;

use devspace_terminal::{PtySession, TerminalEmulator, TerminalSize};
use gpui::{
    div, prelude::*, App, Context, FocusHandle, Focusable, KeyDownEvent, Keystroke, MouseButton,
    Window,
};

use crate::theme::GlassTheme;

/// Fixed grid for now — CLAUDE.md's "cap terminal scrollback" is satisfied
/// by alacritty's default history, but resizing the grid to the panel's
/// actual pixel size isn't wired up yet (see docs/progress.md).
const COLS: usize = 100;
const ROWS: usize = 28;
const POLL_INTERVAL: Duration = Duration::from_millis(16);

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

/// Translates a keystroke into the bytes a shell on the other end of the
/// pty expects: printable characters, Enter/Backspace/Tab/Escape, arrow/
/// Home/End/Delete/Page keys, and ctrl-letter control codes. Not the full
/// xterm modifier matrix (see zed's `terminal::mappings::keys` for that) —
/// expand here if a workflow needs more than plain shell use.
fn keystroke_to_bytes(keystroke: &Keystroke) -> Option<Vec<u8>> {
    if keystroke.modifiers.control && keystroke.key.len() == 1 {
        let c = keystroke.key.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_uppercase() {
            return Some(vec![(c as u8) - b'A' + 1]);
        }
    }
    match keystroke.key.as_str() {
        "enter" => Some(vec![b'\r']),
        "backspace" => Some(vec![0x7f]),
        "tab" => Some(vec![b'\t']),
        "escape" => Some(vec![0x1b]),
        "up" => Some(b"\x1b[A".to_vec()),
        "down" => Some(b"\x1b[B".to_vec()),
        "right" => Some(b"\x1b[C".to_vec()),
        "left" => Some(b"\x1b[D".to_vec()),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        _ => keystroke.key_char.as_ref().map(|s| s.as_bytes().to_vec()),
    }
}

pub struct TerminalView {
    focus_handle: FocusHandle,
    session: Option<PtySession>,
    emulator: TerminalEmulator,
    theme: GlassTheme,
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>, cwd: PathBuf, theme: GlassTheme) -> Self {
        let size = TerminalSize {
            cols: COLS,
            rows: ROWS,
        };
        let session = match PtySession::spawn(&default_shell(), &cwd, size) {
            Ok(session) => Some(session),
            Err(err) => {
                tracing::error!(%err, "failed to spawn terminal shell");
                None
            }
        };

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            if this.update(cx, |view, cx| view.pump(cx)).is_err() {
                break;
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            session,
            emulator: TerminalEmulator::new(size),
            theme,
        }
    }

    /// Drains whatever pty output arrived since the last poll into the
    /// VT100 emulator. Cheap no-op when the shell has been quiet.
    fn pump(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut changed = false;
        while let Some(bytes) = session.try_recv_output() {
            self.emulator.advance(&bytes);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some(bytes) = keystroke_to_bytes(&event.keystroke) {
            let _ = session.write(&bytes);
            cx.notify();
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let focused = self.focus_handle.is_focused(window);
        let lines: Vec<String> = self
            .emulator
            .visible_text()
            .split('\n')
            .map(|s| s.to_string())
            .collect();

        div()
            .id("terminal-view")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }),
            )
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .h_full()
            .p(theme.space_2)
            .rounded(theme.radius_md)
            .bg(theme.surface_solid)
            .border_1()
            .border_color(if focused {
                theme.accent
            } else {
                theme.border_subtle
            })
            .font_family("Consolas")
            .text_sm()
            .text_color(theme.text_primary)
            .overflow_hidden()
            .children(
                lines
                    .into_iter()
                    .map(|line| div().child(line).into_any_element()),
            )
    }
}
