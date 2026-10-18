//! GPUI view for the integrated terminal (CLAUDE.md "Developer tools").
//! The pty + VT100 backend lives in `devspace-terminal`; this entity polls
//! pty output on a background timer so the UI thread never blocks on pty
//! I/O (CLAUDE.md GPUI guidelines), and forwards key presses back to the
//! shell.

use std::path::PathBuf;
use std::time::Duration;

use devspace_terminal::{PtySession, StyledRun, TermColor, TerminalEmulator, TerminalSize};
use gpui::{
    canvas, div, font, prelude::*, px, rgb, App, Bounds, ClipboardItem, Context, FocusHandle,
    Focusable, FontWeight, Hsla, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Window,
};

use crate::theme::GlassTheme;

/// Grid used for the first frame, before the panel has been measured; the
/// canvas in `render` resizes it to the real pixel size right after.
const INITIAL_COLS: usize = 100;
const INITIAL_ROWS: usize = 28;
const POLL_INTERVAL: Duration = Duration::from_millis(16);
const FONT_FAMILY: &str = crate::theme::MONO_FONT;
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 18.0;

fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

/// Translates a keystroke into the bytes a shell on the other end of the
/// pty expects: printable characters, Enter/Backspace/Tab/Escape, arrow/
/// Home/End/Delete/Page keys, ctrl-letter control codes, and alt as an ESC
/// prefix. Not the full xterm modifier matrix (see zed's
/// `terminal::mappings::keys` for that) — expand here if a workflow needs
/// more than plain shell use.
fn keystroke_to_bytes(keystroke: &Keystroke) -> Option<Vec<u8>> {
    let mods = &keystroke.modifiers;
    // Cmd/Win-modified keys are app shortcuts, never shell input.
    if mods.platform {
        return None;
    }
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4). Lets
    // Ctrl+Left/Right jump words, Shift+arrows etc. reach the shell.
    let modifier_param = 1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.control as u8;
    let nav_final = match keystroke.key.as_str() {
        "up" => Some(b'A'),
        "down" => Some(b'B'),
        "right" => Some(b'C'),
        "left" => Some(b'D'),
        "home" => Some(b'H'),
        "end" => Some(b'F'),
        _ => None,
    };
    if let Some(final_byte) = nav_final {
        return Some(if modifier_param == 1 {
            vec![0x1b, b'[', final_byte]
        } else {
            format!("\x1b[1;{modifier_param}{}", final_byte as char).into_bytes()
        });
    }
    if keystroke.modifiers.control && keystroke.key.len() == 1 {
        let c = keystroke.key.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_uppercase() {
            return Some(vec![(c as u8) - b'A' + 1]);
        }
    }
    let bytes = match keystroke.key.as_str() {
        "enter" => Some(vec![b'\r']),
        "backspace" => Some(vec![0x7f]),
        "tab" if keystroke.modifiers.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(vec![b'\t']),
        "escape" => Some(vec![0x1b]),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        _ => keystroke.key_char.as_ref().map(|s| s.as_bytes().to_vec()),
    }?;
    if keystroke.modifiers.alt {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend(bytes);
        return Some(prefixed);
    }
    Some(bytes)
}

fn to_hsla(color: TermColor, default: Hsla) -> Hsla {
    match color {
        TermColor::Default => default,
        TermColor::Rgb(r, g, b) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into(),
    }
}

pub struct TerminalView {
    focus_handle: FocusHandle,
    cwd: PathBuf,
    session: Option<PtySession>,
    emulator: TerminalEmulator,
    theme: GlassTheme,
    exited: bool,
    /// Sub-line remainder of wheel/trackpad scrolling, in pixels.
    scroll_remainder: f32,
    /// Top-left of the text area and one cell's width, captured during
    /// prepaint so mouse positions can be mapped to grid cells.
    origin: Point<Pixels>,
    cell_width: f32,
    selecting: bool,
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>, cwd: PathBuf, theme: GlassTheme) -> Self {
        let size = TerminalSize {
            cols: INITIAL_COLS,
            rows: INITIAL_ROWS,
        };

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            if this.update(cx, |view, cx| view.pump(cx)).is_err() {
                break;
            }
        })
        .detach();

        let mut view = Self {
            focus_handle: cx.focus_handle(),
            cwd,
            session: None,
            emulator: TerminalEmulator::new(size),
            theme,
            exited: false,
            scroll_remainder: 0.0,
            origin: Point::default(),
            cell_width: 1.0,
            selecting: false,
        };
        view.spawn_session();
        view
    }

    fn spawn_session(&mut self) {
        let size = self.emulator.size();
        self.exited = false;
        self.session = match PtySession::spawn(&default_shell(), &self.cwd, size) {
            Ok(session) => Some(session),
            Err(err) => {
                tracing::error!(%err, "failed to spawn terminal shell");
                None
            }
        };
    }

    /// Drains whatever pty output arrived since the last poll into the
    /// VT100 emulator, answers the emulator's queries (ConPTY stalls until
    /// its cursor-position request is answered), and notices shell exit.
    /// Cheap no-op when the shell has been quiet.
    fn pump(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut changed = false;
        while let Some(bytes) = session.try_recv_output() {
            self.emulator.advance(&bytes);
            changed = true;
        }
        let replies = self.emulator.take_pty_replies();
        if !replies.is_empty() {
            let _ = session.write(&replies);
        }
        if !self.exited && session.has_exited() {
            self.exited = true;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Resizes the emulator grid and the pty to fit `viewport`. Called from
    /// the canvas prepaint, so it must be a no-op when nothing changed.
    fn fit_to_viewport(
        &mut self,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
        cx: &mut Context<Self>,
    ) {
        self.origin = bounds.origin;
        self.cell_width = f32::from(cell_width).max(1.0);
        let cols = (f32::from(bounds.size.width) / self.cell_width).floor() as usize;
        let rows = (f32::from(bounds.size.height) / LINE_HEIGHT).floor() as usize;
        let size = TerminalSize {
            cols: cols.max(2),
            rows: rows.max(1),
        };
        let current = self.emulator.size();
        if size.cols == current.cols && size.rows == current.rows {
            return;
        }
        self.emulator.resize(size);
        if let Some(session) = self.session.as_ref() {
            let _ = session.resize(size);
        }
        cx.notify();
    }

    fn write_to_shell(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        self.emulator.scroll_to_bottom();
        let _ = session.write(bytes);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if self.exited {
            // Enter restarts the shell in the same folder.
            if keystroke.key == "enter" {
                self.emulator = TerminalEmulator::new(self.emulator.size());
                self.spawn_session();
                cx.notify();
            }
            return;
        }
        let mods = &keystroke.modifiers;
        let key = keystroke.key.as_str();
        // Cmd on macOS; Ctrl elsewhere, where Ctrl+V/Ctrl+C are the
        // conventional paste/copy (Windows Terminal behaves the same way).
        let command = mods.platform || (mods.control && !cfg!(target_os = "macos"));

        if (command && key == "v") || (mods.shift && key == "insert") {
            self.paste(cx);
            return;
        }
        if command && key == "c" {
            // With a selection this is "copy". Without one, plain Ctrl+C
            // must still reach the shell as an interrupt; Cmd+C and
            // Ctrl+Shift+C never do.
            if self.copy_selection(cx) {
                return;
            }
            if mods.platform || mods.shift {
                return;
            }
        }
        if let Some(bytes) = keystroke_to_bytes(keystroke) {
            self.emulator.clear_selection();
            self.write_to_shell(&bytes, cx);
        }
    }

    /// Copies the selection to the clipboard and clears it. Returns whether
    /// there was anything to copy.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.emulator.selected_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.emulator.clear_selection();
        cx.notify();
        true
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        // Terminals expect CR for line breaks. Strip ESC so pasted text
        // can't smuggle control sequences (or a bracketed-paste end marker)
        // into the shell.
        let text = text
            .replace("\r\n", "\n")
            .replace('\n', "\r")
            .replace('\x1b', "");
        let payload = if self.emulator.bracketed_paste() {
            format!("\x1b[200~{text}\x1b[201~")
        } else {
            text
        };
        self.emulator.clear_selection();
        self.write_to_shell(payload.as_bytes(), cx);
    }

    /// Maps a window-space mouse position to a (col, row) grid cell.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let x = f32::from(position.x - self.origin.x).max(0.0);
        let y = f32::from(position.y - self.origin.y).max(0.0);
        ((x / self.cell_width) as usize, (y / LINE_HEIGHT) as usize)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let (col, row) = self.cell_at(event.position);
        // Double-click selects a word; a single click starts a drag.
        self.emulator
            .start_selection(col, row, event.click_count >= 2);
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let (col, row) = self.cell_at(event.position);
        self.emulator.update_selection(col, row);
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        self.selecting = false;
        // A bare click leaves an empty selection; drop it.
        if self.emulator.selected_text().is_none() {
            self.emulator.clear_selection();
        }
        cx.notify();
    }

    /// Right-click: copy if something is selected, otherwise paste (the
    /// Windows Terminal convention).
    fn on_right_click(&mut self, cx: &mut Context<Self>) {
        if !self.copy_selection(cx) {
            self.paste(cx);
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y);
        self.scroll_remainder += dy;
        let lines = (self.scroll_remainder / LINE_HEIGHT).trunc();
        if lines != 0.0 {
            self.scroll_remainder -= lines * LINE_HEIGHT;
            self.emulator.scroll_lines(lines as i32);
            cx.notify();
        }
    }

    fn render_run(&self, run: &StyledRun, focused: bool) -> gpui::AnyElement {
        let theme = self.theme;
        let mut fg = to_hsla(run.fg, theme.text_primary);
        let mut bg = to_hsla(run.bg, gpui::transparent_black());
        if run.inverse {
            let (new_fg, new_bg) = (
                if run.bg == TermColor::Default {
                    theme.surface_solid
                } else {
                    bg
                },
                if run.fg == TermColor::Default {
                    theme.text_primary
                } else {
                    fg
                },
            );
            fg = new_fg;
            bg = new_bg;
        }
        if run.selected {
            bg = theme.accent.opacity(0.4);
            fg = theme.text_primary;
        }
        if run.cursor {
            // Solid block when focused; dimmer block so an unfocused
            // terminal still shows where the cursor is.
            fg = theme.surface_solid;
            bg = if focused {
                theme.text_primary
            } else {
                theme.text_secondary
            };
        }
        div()
            .whitespace_nowrap()
            .text_color(fg)
            .bg(bg)
            .when(run.bold, |d| d.font_weight(FontWeight::BOLD))
            .when(run.underline, |d| d.underline())
            .child(run.text.clone())
            .into_any_element()
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
        let snapshot = self.emulator.snapshot();
        let view = cx.entity();

        let rows = snapshot.rows.iter().map(|runs| {
            div()
                .flex()
                .flex_row()
                .flex_none()
                .h(px(LINE_HEIGHT))
                .children(runs.iter().map(|run| self.render_run(run, focused)))
        });

        div()
            .id("terminal-view")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(|this, event, _window, cx| this.on_scroll(event, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(|this, event, _window, cx| this.on_mouse_move(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _window, cx| this.on_mouse_up(event, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, _window, cx| this.on_mouse_up(event, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| this.on_right_click(cx)),
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
            .font(font(FONT_FAMILY))
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .text_color(theme.text_primary)
            .overflow_hidden()
            .child(
                // The content box: the canvas measures it so the pty grid
                // matches the pixels available.
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                let text_system = window.text_system();
                                let font_id = text_system.resolve_font(&font(FONT_FAMILY));
                                if let Ok(advance) =
                                    text_system.advance(font_id, px(FONT_SIZE), 'm')
                                {
                                    view.update(cx, |this, cx| {
                                        this.fit_to_viewport(bounds, advance.width, cx)
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(rows)
                    .when(self.exited, |el| {
                        el.child(
                            div()
                                .text_color(theme.text_secondary)
                                .child("[process exited — press Enter to restart]"),
                        )
                    }),
            )
    }
}
