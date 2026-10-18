//! Hands files to the operating system: open with the default application
//! or reveal in the file manager. Arguments are passed as discrete process
//! arguments (never through a shell), so file names can't inject commands.

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
    #[cfg(windows)]
    let mut command = {
        // explorer.exe opens a document with its default handler and, unlike
        // `cmd /c start`, does not interpret shell metacharacters in the path.
        let mut c = Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    // explorer.exe reports exit code 1 even when it succeeded, so only a
    // failure to start the process counts as an error.
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open the file: {e}"))
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
