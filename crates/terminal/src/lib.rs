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
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi;
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

/// VT100/xterm emulation over a byte stream: feed it what a `PtySession`
/// reads and it maintains a screen grid, independent of how those bytes
/// arrived (keeps this half unit-testable without spawning a real shell).
pub struct TerminalEmulator {
    term: Term<EventProxy>,
    parser: ansi::Processor,
    pub events: mpsc::Receiver<TermEvent>,
}

impl TerminalEmulator {
    pub fn new(size: TerminalSize) -> Self {
        let (tx, rx) = mpsc::channel();
        let term = Term::new(TermConfig::default(), &size, EventProxy(tx));
        Self {
            term,
            parser: ansi::Processor::new(),
            events: rx,
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
