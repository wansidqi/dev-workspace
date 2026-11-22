//! Hands files to the operating system: open with the default application
//! or reveal in the file manager. Arguments are passed as discrete process
//! arguments (never through a shell), so file names can't inject commands.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use devspace_core::is_launch_risky;

/// What happened when an attachment was "opened".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenOutcome {
    Opened,
    /// The file type could run code, so it was shown in the file manager
    /// instead of being launched (attachments are untrusted input).
    Revealed,
}

/// Opens `path` with its default application, unless `file_name` is a type
/// that would execute (see [`is_launch_risky`]) — those are revealed instead.
pub fn open_attachment(path: &Path, file_name: &str) -> Result<OpenOutcome, String> {
    if is_launch_risky(file_name) {
        reveal(path)?;
        Ok(OpenOutcome::Revealed)
    } else {
        open(path)?;
        Ok(OpenOutcome::Opened)
    }
}

fn open(path: &Path) -> Result<(), String> {
    open_target(path.as_os_str())
}

fn open_target(target: &OsStr) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = {
        // explorer.exe opens a document with its default handler and, unlike
        // `cmd /c start`, does not interpret shell metacharacters in the path.
        let mut c = Command::new("explorer");
        c.arg(target);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    // explorer.exe reports exit code 1 even when it succeeded, so only a
    // failure to start the process counts as an error.
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open it: {e}"))
}

/// Shows `path` selected in the file manager.
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("explorer");
        // `/select,"<path>"` must reach explorer with exactly this quoting.
        // Windows paths cannot contain double quotes, so this can't be broken
        // out of.
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        // No portable "select" on Linux: open the containing folder.
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't show the file: {e}"))
}

/// True for a link the note preview may hand to the system: `http`, `https` or
/// `mailto`, with no whitespace or control characters. Everything else (file
/// paths, `file:`, `javascript:`, custom schemes) is refused, because the
/// default handler could run code and notes are untrusted input.
pub fn is_openable_url(url: &str) -> bool {
    const MAX_LEN: usize = 2048;
    if url.is_empty() || url.len() > MAX_LEN {
        return false;
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme) && lower.len() > scheme.len())
}

/// Opens a link from a note in the default browser or mail client. The URL is
/// one process argument, never part of a shell string.
pub fn open_url(url: &str) -> Result<(), String> {
    if !is_openable_url(url) {
        return Err("Only http, https and mailto links can be opened".to_string());
    }
    open_target(OsStr::new(url))
}

#[cfg(test)]
mod tests {
    use super::is_openable_url;

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(is_openable_url("https://pages.github.com/"));
        assert!(is_openable_url("HTTP://example.com/a?b=c&d=e#f"));
        assert!(is_openable_url("mailto:someone@example.com"));
    }

    #[test]
    fn anything_that_could_run_code_is_refused() {
        for bad in [
            "",
            "https://",
            "file:///C:/Windows/System32/calc.exe",
            "C:\\Windows\\System32\\calc.exe",
            "javascript:alert(1)",
            "ms-msdt:/id PCWDiagnostic",
            "ftp://example.com",
            "docs/CONTRIBUTING.md",
            "https://example.com/a b",
            "https://example.com/\n",
            "  https://example.com",
        ] {
            assert!(!is_openable_url(bad), "{bad:?} should be refused");
        }
        assert!(!is_openable_url(&format!("https://e.com/{}", "a".repeat(3000))));
    }
}
