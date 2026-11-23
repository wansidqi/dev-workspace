//! Running the system `git` for the work libgit2 can't do well: talking to
//! remotes (so the user's credential manager, SSH config and hooks apply) and
//! rewriting history (merge, rebase, revert, cherry-pick).
//!
//! Rules from CLAUDE.md: arguments are a list, never a shell string; the
//! executable is resolved once; `GIT_TERMINAL_PROMPT=0` makes a missing
//! credential fail instead of hanging; credentials embedded in a remote URL
//! never reach an error message.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::GitError;

/// How long a network command may run before it is killed.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Local commands (merge, rebase) are quick unless a hook is slow.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

static GIT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The git executable, looked up once: `PATH` first, then the places Git for
/// Windows installs to. `None` when git isn't installed.
pub fn git_executable() -> Option<&'static Path> {
    GIT.get_or_init(find_git).as_deref()
}

fn find_git() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "git.exe" } else { "git" };
    let from_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|candidate| candidate.is_file())
    });
    if from_path.is_some() {
        return from_path;
    }
    let fallbacks: &[&str] = if cfg!(windows) {
        &[
            "C:\\Program Files\\Git\\cmd\\git.exe",
            "C:\\Program Files (x86)\\Git\\cmd\\git.exe",
        ]
    } else {
        &[
            "/usr/bin/git",
            "/usr/local/bin/git",
            "/opt/homebrew/bin/git",
        ]
    };
    fallbacks
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

/// What a finished git command printed.
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    /// stderr and stdout joined, credentials scrubbed: the text to show when
    /// a command failed (git reports some failures, like merge conflicts, on
    /// stdout).
    pub fn message(&self) -> String {
        let mut text = self.stderr.trim().to_owned();
        if text.is_empty() {
            text = self.stdout.trim().to_owned();
        }
        scrub(&text)
    }
}

/// Removes `user:password@` from any URL in `text` so a failed fetch never
/// echoes a token embedded in a remote URL.
pub(crate) fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/' || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => out.push_str(&authority[at + 1..]),
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Which kind of command this is, for the timeout and the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Local,
    Network,
}

/// Runs `git <args>` in `dir` with extra environment variables.
pub(crate) fn run(
    dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    kind: Kind,
) -> Result<CliOutput, GitError> {
    let exe = git_executable().ok_or_else(|| {
        GitError::Invalid(
            "git isn't installed (or isn't on PATH). Install Git to fetch, pull, push, merge or rebase."
                .into(),
        )
    })?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A missing credential must fail, not wait for a prompt nobody sees.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        // Never open an editor: merge/rebase/revert keep their default message.
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        // English output, so the few messages we match on are stable.
        .env("LC_ALL", "C");
    if kind == Kind::Network {
        // Fail on a stalled SSH connection rather than hanging for minutes.
        cmd.env(
            "GIT_SSH_COMMAND",
            std::env::var("GIT_SSH_COMMAND")
                .unwrap_or_else(|_| "ssh -o BatchMode=yes -o ConnectTimeout=20".into()),
        );
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up for every command.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let limit = match kind {
        Kind::Local => LOCAL_TIMEOUT,
        Kind::Network => NETWORK_TIMEOUT,
    };
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::Invalid(format!(
                "git took longer than {} minutes and was stopped",
                limit.as_secs() / 60
            )));
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(CliOutput {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// `run`, turning a non-zero exit into a [`GitError::Invalid`] that carries
/// git's own explanation.
pub(crate) fn run_ok(
    dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    kind: Kind,
) -> Result<CliOutput, GitError> {
    let out = run(dir, args, envs, kind)?;
    if out.success {
        Ok(out)
    } else {
        let message = out.message();
        Err(GitError::Invalid(if message.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or(""))
        } else {
            message
        }))
    }
}

/// A full 40 (or 64, for SHA-256 repos) character commit id.
pub(crate) fn is_full_hash(hash: &str) -> bool {
    matches!(hash.len(), 40 | 64) && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_in_urls_are_scrubbed() {
        assert_eq!(
            scrub("fatal: unable to access 'https://user:tok3n@github.com/a/b.git/': 403"),
            "fatal: unable to access 'https://github.com/a/b.git/': 403"
        );
        assert_eq!(
            scrub("https://ghp_abc@host/x and ssh://git@host/y"),
            "https://host/x and ssh://host/y"
        );
        assert_eq!(scrub("no url here"), "no url here");
        assert_eq!(
            scrub("see https://example.com/a@b"),
            "see https://example.com/a@b"
        );
    }

    #[test]
    fn only_full_hex_hashes_are_accepted() {
        assert!(is_full_hash(&"a".repeat(40)));
        assert!(!is_full_hash("abc123"));
        assert!(!is_full_hash(&format!("{}x", "a".repeat(39))));
        assert!(!is_full_hash("--upload-pack=evil"));
    }
}
