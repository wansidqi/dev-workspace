//! Integrated terminal backend: `portable-pty` spawns and owns the shell
//! process, `alacritty_terminal` parses its output into a screen grid. No
//! UI dependencies here — the `app` crate's GPUI views read `visible_text`
//! (and later, cell-level styling) off `TerminalEmulator` on a background
//! poll, never blocking the UI thread on pty I/O (CLAUDE.md GPUI
//! guidelines).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{self, Color, NamedColor};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub use alacritty_terminal::event::Event as TerminalEvent;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("pty error: {0}")]
    Pty(#[from] anyhow::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Fixed-size terminal dimensions, used both for the pty's kernel-side
/// winsize and for `alacritty_terminal`'s grid.
#[derive(Debug, Clone, Copy)]
pub struct TerminalSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone)]
struct EventProxy(mpsc::Sender<TermEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        // The receiver may legitimately be gone (session closed) by the
        // time a trailing event fires; dropping it is not an error.
        let _ = self.0.send(event);
    }
}

/// A spawned shell process on a pty. Owns the master end; the writer and
/// background reader thread are set up once and live for the session.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Raw output bytes from the shell, filled by a background thread so
    /// reading the pty never blocks the caller.
    output_rx: mpsc::Receiver<Vec<u8>>,
    _reader_thread: JoinHandle<()>,
}

impl Drop for PtySession {
    /// Closing a terminal tab must end its shell, not leak the process.
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl PtySession {
    /// Spawns `shell` (e.g. the platform default, see CLAUDE.md
    /// cross-platform notes) in `cwd` on a new pty of the given size.
    pub fn spawn(shell: &str, cwd: &Path, size: TerminalSize) -> Result<Self, TerminalError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: size.rows as u16,
                cols: size.cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(TerminalError::Pty)?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd).map_err(TerminalError::Pty)?;
        // The slave fd isn't needed once the child has inherited it.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(TerminalError::Pty)?;
        let writer = pair.master.take_writer().map_err(TerminalError::Pty)?;

        let (tx, rx) = mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            child,
            output_rx: rx,
            _reader_thread: reader_thread,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), TerminalError> {
        self.writer.write_all(bytes)?;
        Ok(())
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        self.master
            .resize(PtySize {
                rows: size.rows as u16,
                cols: size.cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(TerminalError::Pty)
    }

    /// Drains whatever output has arrived since the last call, without
    /// blocking. Returns `None` once nothing is pending.
    pub fn try_recv_output(&self) -> Option<Vec<u8>> {
        self.output_rx.try_recv().ok()
    }

    /// `true` once the shell process has exited.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

/// Scrollback lines kept per terminal unless the caller says otherwise.
/// alacritty's own default (10k) costs real memory per tab; CLAUDE.md asks
/// for a capped, configurable scrollback.
pub const DEFAULT_SCROLLBACK_LINES: usize = 2_000;

/// A cell colour as the view should paint it. `Default` means "use the
/// theme's terminal foreground/background" so the UI keeps ownership of
/// those two; everything else is resolved to the standard xterm palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermColor {
    Default,
    Rgb(u8, u8, u8),
}

/// A horizontal span of cells sharing one style, so a view can render a row
/// as a handful of text elements instead of one per cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRun {
    pub text: String,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub underline: bool,
    /// Swap fg and bg when painting (SGR 7). Left to the view because
    /// `Default` colours resolve against its theme.
    pub inverse: bool,
    /// This run is the single cell under the cursor.
    pub cursor: bool,
    /// Cells inside the active selection.
    pub selected: bool,
}

/// Styled copy of the visible screen, one `Vec<StyledRun>` per row.
#[derive(Debug, Clone, Default)]
pub struct ScreenSnapshot {
    pub rows: Vec<Vec<StyledRun>>,
}

/// VT100/xterm emulation over a byte stream: feed it what a `PtySession`
/// reads and it maintains a screen grid, independent of how those bytes
/// arrived (keeps this half unit-testable without spawning a real shell).
pub struct TerminalEmulator {
    term: Term<EventProxy>,
    parser: ansi::Processor,
    events: mpsc::Receiver<TermEvent>,
    title: Option<String>,
}

impl TerminalEmulator {
    pub fn new(size: TerminalSize) -> Self {
        Self::with_scrollback(size, DEFAULT_SCROLLBACK_LINES)
    }

    pub fn with_scrollback(size: TerminalSize, scrollback_lines: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        let config = TermConfig {
            scrolling_history: scrollback_lines,
            ..TermConfig::default()
        };
        let term = Term::new(config, &size, EventProxy(tx));
        Self {
            term,
            parser: ansi::Processor::new(),
            events: rx,
            title: None,
        }
    }

    /// Drains emulator events. Returns the bytes the emulator wants sent
    /// back to the shell (cursor-position and device-status replies, which
    /// Windows ConPTY blocks on) — the caller must write them to the pty.
    /// Title changes are remembered and available via [`Self::title`].
    pub fn take_pty_replies(&mut self) -> Vec<u8> {
        let mut replies = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                TermEvent::PtyWrite(text) => replies.extend_from_slice(text.as_bytes()),
                TermEvent::Title(title) => self.title = Some(title),
                TermEvent::ResetTitle => self.title = None,
                _ => {}
            }
        }
        replies
    }

    /// Window title set by the shell via OSC 0/2, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Scrolls the viewport through scrollback; positive moves toward
    /// older output.
    pub fn scroll_lines(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn viewport_point(&self, col: usize, row: usize) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        let col = col.min(self.term.columns().saturating_sub(1));
        let row = row.min(self.term.screen_lines().saturating_sub(1));
        Point::new(Line(row as i32 - offset), Column(col))
    }

    /// Begins a selection at a viewport cell. `word` selects whole words
    /// (double-click), otherwise it's a plain character range.
    pub fn start_selection(&mut self, col: usize, row: usize, word: bool) {
        let kind = if word {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        let point = self.viewport_point(col, row);
        self.term.selection = Some(Selection::new(kind, point, Side::Left));
    }

    /// Extends the active selection to a viewport cell (mouse drag).
    pub fn update_selection(&mut self, col: usize, row: usize) {
        let point = self.viewport_point(col, row);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, Side::Right);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Selected text, or `None` when there is no (or an empty) selection —
    /// callers use that to decide whether Ctrl+C means copy or interrupt.
    pub fn selected_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// Whether the shell asked for bracketed paste (`ESC[?2004h`), in which
    /// case pasted text must be wrapped so the shell doesn't execute it
    /// line by line.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Snaps the viewport back to the live screen (called on keyboard input).
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn size(&self) -> TerminalSize {
        TerminalSize {
            cols: self.term.columns(),
            rows: self.term.screen_lines(),
        }
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, size: TerminalSize) {
        self.term.resize(size);
    }

    /// Plain-text snapshot of the visible screen, one row per line with
    /// trailing blanks trimmed. For styled rendering, callers will need
    /// cell-level access (`Term::grid`) instead — this is enough for
    /// "did the shell produce the output we expected" style tests.
    pub fn visible_text(&self) -> String {
        let grid = self.term.grid();
        let mut lines = Vec::with_capacity(grid.screen_lines());
        for row_idx in 0..grid.screen_lines() {
            let row = &grid[Line(row_idx as i32)];
            let text: String = row.into_iter().map(|cell| cell.c).collect();
            lines.push(text.trim_end().to_string());
        }
        lines.join("\n")
    }

    /// Styled snapshot of the viewport for rendering, honouring scrollback
    /// position. The cursor is only reported when viewing the live screen
    /// and the shell hasn't hidden it.
    pub fn snapshot(&self) -> ScreenSnapshot {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let cursor = (offset == 0 && self.term.mode().contains(TermMode::SHOW_CURSOR))
            .then_some(grid.cursor.point);
        let cols = grid.columns();
        let selection = self
            .term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term));
        let mut rows = Vec::with_capacity(grid.screen_lines());

        for row_idx in 0..grid.screen_lines() {
            let line = Line(row_idx as i32 - offset);
            let row = &grid[line];
            let mut runs: Vec<StyledRun> = Vec::new();
            for col in 0..cols {
                let cell = &row[Column(col)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let is_cursor = cursor.is_some_and(|p| p.line == line && p.column.0 == col);
                let ch = if cell.flags.contains(Flags::HIDDEN) {
                    ' '
                } else {
                    cell.c
                };
                let selected = selection.is_some_and(|r| r.contains(Point::new(line, Column(col))));
                let fg = resolve_color(cell.fg);
                let bg = resolve_color(cell.bg);
                let bold = cell.flags.intersects(Flags::BOLD);
                let underline = cell.flags.intersects(Flags::UNDERLINE);
                let inverse = cell.flags.contains(Flags::INVERSE);
                match runs.last_mut() {
                    Some(run)
                        if !run.cursor
                            && !is_cursor
                            && run.fg == fg
                            && run.bg == bg
                            && run.bold == bold
                            && run.underline == underline
                            && run.inverse == inverse
                            && run.selected == selected =>
                    {
                        run.text.push(ch);
                    }
                    _ => runs.push(StyledRun {
                        text: ch.to_string(),
                        fg,
                        bg,
                        bold,
                        underline,
                        inverse,
                        cursor: is_cursor,
                        selected,
                    }),
                }
            }
            // Trailing unstyled blanks only cost layout work.
            while let Some(last) = runs.last() {
                let blank = !last.cursor
                    && last.bg == TermColor::Default
                    && !last.inverse
                    && !last.selected
                    && last.text.chars().all(|c| c == ' ');
                if blank {
                    runs.pop();
                } else {
                    break;
                }
            }
            rows.push(runs);
        }
        ScreenSnapshot { rows }
    }
}

/// Standard xterm 16-colour palette (muted, readable on the dark glass
/// surface). Indices 16..=255 follow the xterm 6x6x6 cube and grey ramp.
const ANSI_16: [(u8, u8, u8); 16] = [
    (0x1e, 0x1e, 0x1e),
    (0xcd, 0x31, 0x31),
    (0x0d, 0xbc, 0x79),
    (0xe5, 0xe5, 0x10),
    (0x24, 0x72, 0xc8),
    (0xbc, 0x3f, 0xbc),
    (0x11, 0xa8, 0xcd),
    (0xe5, 0xe5, 0xe5),
    (0x66, 0x66, 0x66),
    (0xf1, 0x4c, 0x4c),
    (0x23, 0xd1, 0x8b),
    (0xf5, 0xf5, 0x43),
    (0x3b, 0x8e, 0xea),
    (0xd6, 0x70, 0xd6),
    (0x29, 0xb8, 0xdb),
    (0xff, 0xff, 0xff),
];

fn indexed_color(i: u8) -> TermColor {
    let i = i as usize;
    let (r, g, b) = match i {
        0..=15 => ANSI_16[i],
        16..=231 => {
            let n = i - 16;
            let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let v = ((i - 232) * 10 + 8) as u8;
            (v, v, v)
        }
    };
    TermColor::Rgb(r, g, b)
}

fn resolve_color(color: Color) -> TermColor {
    match color {
        Color::Spec(rgb) => TermColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => indexed_color(i),
        Color::Named(named) => {
            let n = named as usize;
            match named {
                _ if n < 16 => indexed_color(n as u8),
                NamedColor::DimBlack => indexed_color(0),
                NamedColor::DimRed => indexed_color(1),
                NamedColor::DimGreen => indexed_color(2),
                NamedColor::DimYellow => indexed_color(3),
                NamedColor::DimBlue => indexed_color(4),
                NamedColor::DimMagenta => indexed_color(5),
                NamedColor::DimCyan => indexed_color(6),
                NamedColor::DimWhite => indexed_color(7),
                _ => TermColor::Default,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn emulator_renders_plain_text() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 2 });
        emulator.advance(b"hello");
        assert_eq!(emulator.visible_text(), "hello\n");
    }

    #[test]
    fn emulator_handles_cursor_movement_and_newline() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 2 });
        emulator.advance(b"hi\r\nbye");
        assert_eq!(emulator.visible_text(), "hi\nbye");
    }

    #[test]
    fn emulator_resize_changes_screen_lines() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 2 });
        emulator.resize(TerminalSize { cols: 10, rows: 4 });
        assert_eq!(emulator.term.grid().screen_lines(), 4);
    }

    #[test]
    fn emulator_answers_cursor_position_query() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 3 });
        emulator.advance(b"ab\x1b[6n");
        assert_eq!(emulator.take_pty_replies(), b"\x1b[1;3R");
        assert!(emulator.take_pty_replies().is_empty());
    }

    #[test]
    fn emulator_records_title() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 3 });
        emulator.advance(b"\x1b]0;my shell\x07");
        emulator.take_pty_replies();
        assert_eq!(emulator.title(), Some("my shell"));
    }

    #[test]
    fn snapshot_carries_colour_and_cursor() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 2 });
        emulator.advance(b"\x1b[31mred\x1b[0m ok");
        let snap = emulator.snapshot();
        let row = &snap.rows[0];
        assert_eq!(row[0].text, "red");
        assert_eq!(row[0].fg, TermColor::Rgb(0xcd, 0x31, 0x31));
        assert_eq!(row[1].text, " ok");
        assert_eq!(row[1].fg, TermColor::Default);
        assert!(row.iter().any(|r| r.cursor));
    }

    #[test]
    fn snapshot_hides_cursor_when_shell_hides_it() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 2 });
        emulator.advance(b"\x1b[?25lhi");
        let snap = emulator.snapshot();
        assert!(snap.rows.iter().flatten().all(|r| !r.cursor));
    }

    #[test]
    fn scrollback_is_capped_and_scrollable() {
        let mut emulator = TerminalEmulator::with_scrollback(TerminalSize { cols: 10, rows: 2 }, 5);
        for i in 0..20 {
            emulator.advance(format!("line{i}\r\n").as_bytes());
        }
        let text_of = |e: &TerminalEmulator| -> String {
            e.snapshot()
                .rows
                .iter()
                .flatten()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
                .join("|")
        };
        emulator.scroll_lines(3);
        assert!(!emulator.snapshot().rows.iter().flatten().any(|r| r.cursor));
        emulator.scroll_lines(1000);
        let oldest = text_of(&emulator);
        assert!(
            !oldest.contains("line0|"),
            "history beyond the cap is dropped: {oldest}"
        );
        emulator.scroll_to_bottom();
        assert!(emulator.snapshot().rows.iter().flatten().any(|r| r.cursor));
    }

    #[test]
    fn selection_copies_text_and_marks_cells() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 20, rows: 2 });
        emulator.advance(b"hello world");
        assert_eq!(emulator.selected_text(), None);
        emulator.start_selection(0, 0, false);
        emulator.update_selection(4, 0);
        assert_eq!(emulator.selected_text().as_deref(), Some("hello"));
        let snap = emulator.snapshot();
        let selected: String = snap.rows[0]
            .iter()
            .filter(|r| r.selected)
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(selected, "hello");
        emulator.clear_selection();
        assert_eq!(emulator.selected_text(), None);
    }

    #[test]
    fn double_click_selects_a_word() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 20, rows: 2 });
        emulator.advance(b"hello world");
        emulator.start_selection(7, 0, true);
        assert_eq!(emulator.selected_text().as_deref(), Some("world"));
    }

    #[test]
    fn bracketed_paste_mode_is_tracked() {
        let mut emulator = TerminalEmulator::new(TerminalSize { cols: 10, rows: 2 });
        assert!(!emulator.bracketed_paste());
        emulator.advance(b"\x1b[?2004h");
        assert!(emulator.bracketed_paste());
    }

    #[test]
    fn pty_session_runs_a_command_and_produces_output() {
        let shell = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" };
        let mut session = PtySession::spawn(
            shell,
            &std::env::temp_dir(),
            TerminalSize { cols: 80, rows: 24 },
        )
        .expect("shell must be available to run this test");

        let command = if cfg!(windows) {
            "echo devspace-terminal-test\r\n"
        } else {
            "echo devspace-terminal-test\n"
        };
        session.write(command.as_bytes()).unwrap();

        let mut collected = Vec::new();
        let mut answered_cursor_query = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(chunk) = session.try_recv_output() {
                collected.extend_from_slice(&chunk);
                // ConPTY's handshake queries the cursor position (DEC
                // DSR, `ESC [ 6 n`) before the shell will process further
                // input; a real terminal emulator answers it, so we do
                // too, once, with an arbitrary-but-valid position report.
                if !answered_cursor_query && collected.ends_with(b"\x1b[6n") {
                    answered_cursor_query = true;
                    session.write(b"\x1b[1;1R").unwrap();
                }
                if String::from_utf8_lossy(&collected).contains("devspace-terminal-test") {
                    break;
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        let output = String::from_utf8_lossy(&collected);
        assert!(
            output.contains("devspace-terminal-test"),
            "expected echoed marker in pty output, got: {output:?}"
        );
    }
}
