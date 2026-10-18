//! Runs a user's saved command line through the platform shell and keeps a
//! bounded, sanitised log of its combined stdout/stderr. No UI and no
//! third-party dependencies, so it is testable with real short commands.
//!
//! The only thing ever handed to the shell is the command text the user
//! saved; nothing else is interpolated into it.

mod log;
mod sanitize;

pub use log::LogBuffer;
pub use sanitize::sanitize_line;

use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread;

/// Default number of log lines kept per command.
pub const DEFAULT_LOG_LINES: usize = 5_000;
/// Longest line kept, in characters; the rest is replaced by an ellipsis.
pub const MAX_LINE_CHARS: usize = 2_000;
/// Raw bytes read for one line before the remainder is discarded, so a
/// process printing without newlines cannot grow memory without bound.
const MAX_RAW_LINE_BYTES: usize = 16 * 1024;
/// Lines buffered between reader threads and the UI poll. Bounded so a
/// flooding process is slowed down instead of filling memory.
const CHANNEL_LINES: usize = 2_048;

/// Lifecycle of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// The process is still alive.
    Running,
    /// It ended by itself; `None` when killed by a signal (no exit code).
    Exited(Option<i32>),
    /// The user stopped it via [`RunningCommand::stop`].
    Stopped,
}

/// A started shell command with its live log. Dropping it kills the whole
/// process tree so nothing is orphaned when the app quits.
pub struct RunningCommand {
    child: Child,
    rx: Receiver<String>,
    log: LogBuffer,
    state: RunState,
}

impl RunningCommand {
    /// Starts `command` through the platform shell (`cmd /C` on Windows,
    /// `sh -c` elsewhere) in `cwd`, with stdout and stderr merged into the
    /// log. Fails with `NotFound` if `cwd` is not an existing directory, so
    /// the caller can show a clear message instead of a vague spawn error.
    pub fn spawn(command: &str, cwd: &Path) -> io::Result<Self> {
        Self::spawn_with_capacity(command, cwd, DEFAULT_LOG_LINES)
    }

    /// Like [`spawn`](Self::spawn) with an explicit log line capacity.
    pub fn spawn_with_capacity(command: &str, cwd: &Path, log_lines: usize) -> io::Result<Self> {
        if !cwd.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("working directory does not exist: {}", cwd.display()),
            ));
        }
        let mut cmd = shell_command(command);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;

        let (tx, rx) = sync_channel::<String>(CHANNEL_LINES);
        if let Some(out) = child.stdout.take() {
            spawn_reader(out, tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_reader(err, tx.clone());
        }
        drop(tx);
        Ok(Self {
            child,
            rx,
            log: LogBuffer::new(log_lines),
            state: RunState::Running,
        })
    }

    /// Moves any output produced since the last call into the log without
    /// blocking. Returns how many lines were added.
    pub fn drain_output(&mut self) -> usize {
        let mut added = 0;
        // Bound the work done per UI tick.
        while added < CHANNEL_LINES {
            let Ok(line) = self.rx.try_recv() else {
                break;
            };
            self.log.push(line);
            added += 1;
        }
        added
    }

    /// The captured log.
    pub fn log(&self) -> &LogBuffer {
        &self.log
    }

    /// Empties the log (the process keeps running).
    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    /// Current state; notices a natural exit without blocking.
    pub fn state(&mut self) -> RunState {
        if self.state == RunState::Running {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.state = RunState::Exited(status.code());
            }
        }
        self.state
    }

    /// True while the process is alive.
    pub fn is_running(&mut self) -> bool {
        self.state() == RunState::Running
    }

    /// The exit code once finished on its own (`None` while running, after
    /// a stop, or if killed by a signal).
    pub fn exit_status(&mut self) -> Option<i32> {
        match self.state() {
            RunState::Exited(code) => code,
            _ => None,
        }
    }

    /// Kills the process and everything it started, then marks the run as
    /// stopped. Does nothing if it already ended.
    pub fn stop(&mut self) {
        if self.state() != RunState::Running {
            return;
        }
        kill_tree(&mut self.child);
        let _ = self.child.wait();
        self.state = RunState::Stopped;
        // Pick up what the readers had already captured.
        self.drain_output();
    }
}

impl Drop for RunningCommand {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            kill_tree(&mut self.child);
            let _ = self.child.wait();
        }
    }
}

/// Builds the shell invocation. The command text is passed as the shell's
/// own argument and never combined with anything else.
#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let mut cmd = Command::new("cmd");
    // `/S` makes cmd strip exactly the outer quotes, so inner quotes in the
    // user's command survive; raw_arg avoids Rust re-escaping them.
    cmd.raw_arg("/S /C").raw_arg(format!("\"{command}\""));
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    // Own process group so stop() can signal the whole tree.
    cmd.process_group(0);
    cmd
}

#[cfg(windows)]
fn kill_tree(child: &mut Child) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &child.id().to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(not(windows))]
fn kill_tree(child: &mut Child) {
    // The child leads its own group (see shell_command), so -PID addresses
    // the whole tree.
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

fn spawn_reader<R: Read + Send + 'static>(reader: R, tx: SyncSender<String>) {
    thread::spawn(move || pump_lines(reader, &tx));
}

/// Splits a byte stream into sanitised lines. `\n`, `\r\n` and lone `\r`
/// (progress bars) all end a line; over-long lines are cut.
fn pump_lines<R: Read>(mut reader: R, tx: &SyncSender<String>) {
    let mut chunk = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut overflow = false;
    let mut after_cr = false;
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &chunk[..n] {
            if after_cr {
                after_cr = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\n' | b'\r' => {
                    after_cr = b == b'\r';
                    if !emit(&mut line, &mut overflow, tx) {
                        return;
                    }
                }
                _ if line.len() < MAX_RAW_LINE_BYTES => line.push(b),
                _ => overflow = true,
            }
        }
    }
    if !line.is_empty() {
        emit(&mut line, &mut overflow, tx);
    }
}

/// Sends the buffered line; false when the receiver is gone.
fn emit(line: &mut Vec<u8>, overflow: &mut bool, tx: &SyncSender<String>) -> bool {
    let mut text = sanitize_line(&String::from_utf8_lossy(line), MAX_LINE_CHARS);
    if *overflow && !text.ends_with('\u{2026}') {
        text.push('\u{2026}');
    }
    line.clear();
    *overflow = false;
    tx.send(text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn cwd() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    /// Polls until the command leaves `Running` and its output is drained.
    fn finish(cmd: &mut RunningCommand) -> RunState {
        let deadline = Instant::now() + Duration::from_secs(30);
        while cmd.is_running() {
            assert!(Instant::now() < deadline, "command did not finish");
            cmd.drain_output();
            thread::sleep(Duration::from_millis(20));
        }
        // Readers may lag the exit slightly; wait for the pipes to close.
        let mut idle = 0;
        while idle < 5 {
            if cmd.drain_output() == 0 {
                idle += 1;
                thread::sleep(Duration::from_millis(30));
            } else {
                idle = 0;
            }
        }
        cmd.state()
    }

    fn text(cmd: &RunningCommand) -> String {
        cmd.log().to_text()
    }

    #[test]
    fn captures_output_and_exit_zero() {
        let mut cmd = RunningCommand::spawn("echo hello-runner", &cwd()).unwrap();
        assert_eq!(finish(&mut cmd), RunState::Exited(Some(0)));
        assert!(text(&cmd).contains("hello-runner"), "{}", text(&cmd));
        assert_eq!(cmd.exit_status(), Some(0));
    }

    #[test]
    fn reports_a_failing_exit_code() {
        let mut cmd = RunningCommand::spawn("exit 3", &cwd()).unwrap();
        assert_eq!(finish(&mut cmd), RunState::Exited(Some(3)));
    }

    #[test]
    fn merges_stderr_into_the_log() {
        let mut cmd = RunningCommand::spawn("echo to-err 1>&2", &cwd()).unwrap();
        finish(&mut cmd);
        assert!(text(&cmd).contains("to-err"), "{}", text(&cmd));
    }

    #[test]
    fn missing_cwd_is_a_clear_error() {
        let missing = cwd().join("devspace-runner-definitely-missing-dir");
        let err = RunningCommand::spawn("echo x", &missing).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("working directory"));
    }

    #[cfg(windows)]
    const LOTS: &str = "for /L %i in (1,1,300) do @echo line%i";
    #[cfg(not(windows))]
    const LOTS: &str = "i=1; while [ $i -le 300 ]; do echo line$i; i=$((i+1)); done";

    #[test]
    fn long_output_is_capped_to_the_newest_lines() {
        let mut cmd = RunningCommand::spawn_with_capacity(LOTS, &cwd(), 50).unwrap();
        finish(&mut cmd);
        assert_eq!(cmd.log().len(), 50);
        assert!(cmd.log().dropped() >= 250);
        assert_eq!(cmd.log().get(49), Some("line300"));
        assert_eq!(cmd.log().get(0), Some("line251"));
    }

    #[cfg(windows)]
    const SLEEP: &str = "ping -n 60 127.0.0.1 >nul";
    #[cfg(not(windows))]
    const SLEEP: &str = "sleep 60";

    #[test]
    fn stop_kills_a_sleeping_process() {
        let mut cmd = RunningCommand::spawn(SLEEP, &cwd()).unwrap();
        assert!(cmd.is_running());
        let started = Instant::now();
        cmd.stop();
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(cmd.state(), RunState::Stopped);
        assert!(!cmd.is_running());
        assert_eq!(cmd.exit_status(), None);
        // Stopping again is harmless.
        cmd.stop();
    }

    #[test]
    fn dropping_kills_the_process() {
        let cmd = RunningCommand::spawn(SLEEP, &cwd()).unwrap();
        let started = Instant::now();
        drop(cmd);
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn control_characters_never_reach_the_log() {
        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"[Console]::Out.Write([string][char]27+'[31mred'+[char]27+'[0m'+[char]7+'ok')\"";
        #[cfg(not(windows))]
        let command = "printf '\\033[31mred\\033[0m\\007ok'";
        let mut cmd = RunningCommand::spawn(command, &cwd()).unwrap();
        finish(&mut cmd);
        let log = text(&cmd);
        assert!(log.contains("redok"), "{log:?}");
        assert!(!log.chars().any(|c| c.is_control() && c != '\n'), "{log:?}");
    }

    #[test]
    fn pump_splits_lines_and_caps_runaway_lines() {
        let (tx, rx) = sync_channel(16);
        let mut data = b"a\r\nb\rc\n".to_vec();
        data.extend(std::iter::repeat_n(b'x', MAX_RAW_LINE_BYTES * 2));
        data.extend_from_slice(b"\ntail");
        pump_lines(io::Cursor::new(data), &tx);
        drop(tx);
        let lines: Vec<String> = rx.iter().collect();
        assert_eq!(&lines[..3], ["a", "b", "c"]);
        assert_eq!(lines[3].chars().count(), MAX_LINE_CHARS + 1);
        assert!(lines[3].ends_with('\u{2026}'));
        assert_eq!(lines[4], "tail");
    }
}
