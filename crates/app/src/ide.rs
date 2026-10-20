//! "Open in IDE": launches the user's configured editor command on the
//! project folder.
//!
//! Security approach (see also `external.rs`): the command is split into a
//! program and fixed arguments by [`split_command`], the program is located
//! by our own PATH/PATHEXT lookup, and the process is spawned directly with
//! the folder as ONE discrete argument. No shell is ever involved and the
//! folder is never interpolated into a command line.
//!
//! Windows note: `code` is a `code.cmd` shim, which cannot be executed
//! without `cmd.exe`. We resolve it through PATHEXT ourselves and hand the
//! resolved file to `Command`; Rust's standard library then runs batch files
//! with its hardened argument escaping. `cmd.exe` still expands `%VAR%` in a
//! batch file's arguments, so for `.cmd`/`.bat` launchers a folder whose
//! name contains cmd metacharacters is refused with a message (point the IDE
//! command at the real `.exe` to open such folders).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A launch ready to spawn: resolved program, fixed arguments from the
/// configured command, and the folder as the final argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Why a command line could not be split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    Empty,
    UnterminatedQuote,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "The IDE command is empty"),
            Self::UnterminatedQuote => write!(f, "The IDE command has an unclosed quote"),
        }
    }
}

/// Splits a command into words. Whitespace separates words; single or
/// double quotes group (and may appear mid-word, as in `--folder="a b"`).
/// There are no escape characters, so Windows paths like
/// `"C:\Program Files\Code\Code.exe"` work as written.
pub fn split_command(command: &str) -> Result<Vec<String>, SplitError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(SplitError::UnterminatedQuote);
    }
    if in_word {
        words.push(current);
    }
    if words.is_empty() || words[0].is_empty() {
        return Err(SplitError::Empty);
    }
    Ok(words)
}

/// Finds `program` the way a shell would, without a shell: a name with a
/// path separator is used as given; a bare name is searched for in
/// `search_dirs`. `extensions` (PATHEXT on Windows, empty elsewhere, each
/// with a leading dot) are tried when the name has no extension of its own.
pub fn resolve_program(
    program: &str,
    search_dirs: &[PathBuf],
    extensions: &[String],
) -> Option<PathBuf> {
    let has_separator = program.contains('/') || program.contains('\\');
    let has_extension = Path::new(program).extension().is_some();
    let try_in = |dir: Option<&Path>| -> Option<PathBuf> {
        let base = match dir {
            Some(d) => d.join(program),
            None => PathBuf::from(program),
        };
        let mut candidates = vec![base.clone()];
        if !has_extension {
            for ext in extensions {
                let mut name = base.as_os_str().to_owned();
                name.push(ext);
                candidates.push(PathBuf::from(name));
            }
        }
        // Prefer the extension forms on Windows: a bare `code` (a shell
        // script for other platforms) is not runnable there.
        if !extensions.is_empty() && !has_extension {
            candidates.rotate_left(1);
        }
        candidates.into_iter().find(|p| p.is_file())
    };
    if has_separator {
        try_in(None)
    } else {
        search_dirs.iter().find_map(|dir| try_in(Some(dir)))
    }
}

/// Builds the launch for `command` on `folder`, resolving the program from
/// `search_dirs`/`extensions`. Pure apart from file-existence checks, so it
/// is testable with fake launchers in a temp dir.
pub fn plan_launch(
    command: &str,
    folder: &Path,
    search_dirs: &[PathBuf],
    extensions: &[String],
) -> Result<IdeLaunch, String> {
    if !folder.is_dir() {
        return Err(format!(
            "The project folder does not exist: {}",
            folder.display()
        ));
    }
    plan_target(command, folder, search_dirs, extensions)
}

/// Builds the launch that opens one file of `root` (a path relative to it,
/// as git reports changed files). The path must stay inside `root` and name
/// an existing file, and it reaches the editor as one discrete argument.
pub fn plan_open_file(
    command: &str,
    root: &Path,
    relative: &str,
    search_dirs: &[PathBuf],
    extensions: &[String],
) -> Result<IdeLaunch, String> {
    let inside = !relative.is_empty()
        && Path::new(relative)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !inside {
        return Err(format!("Not a path inside the repository: {relative}"));
    }
    let target = root.join(relative);
    if !target.is_file() {
        return Err(format!(
            "{relative} doesn't exist on disk (was it deleted?)"
        ));
    }
    plan_target(command, &target, search_dirs, extensions)
}

/// The IDE command with `target` appended as its final argument.
fn plan_target(
    command: &str,
    target: &Path,
    search_dirs: &[PathBuf],
    extensions: &[String],
) -> Result<IdeLaunch, String> {
    let mut words = split_command(command).map_err(|e| e.to_string())?;
    let program_word = words.remove(0);
    let program = resolve_program(&program_word, search_dirs, extensions).ok_or_else(|| {
        format!("Couldn't find \"{program_word}\" — check the IDE command and your PATH")
    })?;
    if is_batch_file(&program) && has_cmd_metacharacters(&target.to_string_lossy()) {
        return Err("This path can't be passed safely through a .cmd launcher; \
             set the IDE command to the editor's .exe instead"
            .to_string());
    }
    let mut args: Vec<OsString> = words.into_iter().map(OsString::from).collect();
    args.push(target.as_os_str().to_owned());
    Ok(IdeLaunch { program, args })
}

fn is_batch_file(program: &Path) -> bool {
    program
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// Characters `cmd.exe` treats specially even when arguments are quoted.
fn has_cmd_metacharacters(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '%' | '"' | '^' | '&' | '|' | '<' | '>' | '\n' | '\r' | '!'
        )
    })
}

/// The directories and extensions to search, from the process environment.
fn environment_lookup() -> (Vec<PathBuf>, Vec<String>) {
    let dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let extensions = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|e| e.starts_with('.') && e.len() > 1)
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    (dirs, extensions)
}

/// Launches `command` on `folder` without waiting for the editor to exit.
/// Errors are messages fit for the UI.
pub fn open_in_ide(command: &str, folder: &Path) -> Result<(), String> {
    let (dirs, extensions) = environment_lookup();
    let launch = plan_launch(command, folder, &dirs, &extensions)?;
    spawn_detached(launch)
}

/// Opens one file of `root` in the IDE (see [`plan_open_file`]).
pub fn open_file_in_ide(command: &str, root: &Path, relative: &str) -> Result<(), String> {
    let (dirs, extensions) = environment_lookup();
    let launch = plan_open_file(command, root, relative, &dirs, &extensions)?;
    spawn_detached(launch)
}

fn spawn_detached(launch: IdeLaunch) -> Result<(), String> {
    let mut process = std::process::Command::new(&launch.program);
    process
        .args(&launch.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up for the .cmd shim.
        process.creation_flags(0x0800_0000);
    }
    let mut child = process
        .spawn()
        .map_err(|e| format!("Couldn't start the IDE: {e}"))?;
    // Reap the child when it exits so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        split_command(s).unwrap()
    }

    #[test]
    fn splits_plain_and_quoted_words() {
        assert_eq!(words("code"), ["code"]);
        assert_eq!(
            words("  code   --reuse-window  "),
            ["code", "--reuse-window"]
        );
        assert_eq!(
            words(r#""C:\Program Files\Code\Code.exe" --new-window"#),
            [r"C:\Program Files\Code\Code.exe", "--new-window"]
        );
        assert_eq!(words("idea --opt='a b' x"), ["idea", "--opt=a b", "x"]);
        assert_eq!(words(r#"ed "" x"#), ["ed", "", "x"]);
        assert_eq!(words(r#"a"b c"d"#), ["ab cd"]);
    }

    #[test]
    fn rejects_empty_and_unterminated() {
        assert_eq!(split_command(""), Err(SplitError::Empty));
        assert_eq!(split_command("   \t "), Err(SplitError::Empty));
        assert_eq!(split_command("\"\""), Err(SplitError::Empty));
        assert_eq!(
            split_command("code \"oops"),
            Err(SplitError::UnterminatedQuote)
        );
        assert_eq!(
            split_command("code 'oops"),
            Err(SplitError::UnterminatedQuote)
        );
    }

    #[test]
    fn shell_metacharacters_are_just_text() {
        // Nothing is interpreted: they stay inside words and reach the
        // process verbatim (which never goes through a shell).
        assert_eq!(words("code ; rm -rf /"), ["code", ";", "rm", "-rf", "/"]);
        assert_eq!(words("code $(whoami)"), ["code", "$(whoami)"]);
    }

    struct Dirs(PathBuf);
    impl Dirs {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("devspace-ide-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_bare_names_through_the_search_path() {
        let d = Dirs::new("resolve");
        let bin = d.0.join("bin");
        let other = d.0.join("other");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(bin.join("myide"), "").unwrap();
        let dirs = vec![other.clone(), bin.clone()];
        assert_eq!(
            resolve_program("myide", &dirs, &[]),
            Some(bin.join("myide"))
        );
        assert_eq!(resolve_program("missing", &dirs, &[]), None);
    }

    #[test]
    fn resolves_shims_through_pathext_and_prefers_them() {
        let d = Dirs::new("pathext");
        std::fs::write(d.0.join("code"), "#!/bin/sh").unwrap();
        std::fs::write(d.0.join("code.cmd"), "@echo off").unwrap();
        let exts = vec![".EXE".to_string(), ".cmd".to_string()];
        let found = resolve_program("code", std::slice::from_ref(&d.0), &exts).unwrap();
        assert_eq!(found, d.0.join("code.cmd"));
        // A name that already has an extension is taken as is.
        std::fs::write(d.0.join("tool.exe"), "").unwrap();
        assert_eq!(
            resolve_program("tool.exe", std::slice::from_ref(&d.0), &exts),
            Some(d.0.join("tool.exe"))
        );
    }

    #[test]
    fn explicit_paths_are_not_searched() {
        let d = Dirs::new("explicit");
        std::fs::write(d.0.join("ide.exe"), "").unwrap();
        let full = d.0.join("ide.exe");
        assert_eq!(
            resolve_program(&full.to_string_lossy(), &[], &[]),
            Some(full)
        );
        assert_eq!(
            resolve_program("./nope/ide", std::slice::from_ref(&d.0), &[]),
            None
        );
    }

    #[test]
    fn the_folder_is_always_one_discrete_final_argument() {
        let d = Dirs::new("plan");
        std::fs::write(d.0.join("fakeide"), "").unwrap();
        let tricky = d.0.join("my proj; echo pwned $(x) 'q'");
        std::fs::create_dir_all(&tricky).unwrap();
        let plan = plan_launch(
            "fakeide --reuse-window \"--profile=a b\"",
            &tricky,
            std::slice::from_ref(&d.0),
            &[],
        )
        .unwrap();
        assert_eq!(plan.program, d.0.join("fakeide"));
        assert_eq!(
            plan.args,
            vec![
                OsString::from("--reuse-window"),
                OsString::from("--profile=a b"),
                tricky.as_os_str().to_owned()
            ]
        );
    }

    #[test]
    fn rejects_bad_commands_and_missing_folders_with_clear_messages() {
        let d = Dirs::new("errors");
        std::fs::write(d.0.join("fakeide"), "").unwrap();
        let dirs = [d.0.clone()];
        let empty = plan_launch("  ", &d.0, &dirs, &[]).unwrap_err();
        assert!(empty.contains("empty"), "{empty}");
        let missing_dir = plan_launch("fakeide", &d.0.join("nope"), &dirs, &[]).unwrap_err();
        assert!(missing_dir.contains("does not exist"), "{missing_dir}");
        let unknown = plan_launch("no-such-ide", &d.0, &dirs, &[]).unwrap_err();
        assert!(unknown.contains("Couldn't find"), "{unknown}");
        let quote = plan_launch("fakeide \"x", &d.0, &dirs, &[]).unwrap_err();
        assert!(quote.contains("quote"), "{quote}");
    }

    #[test]
    fn a_file_is_one_discrete_final_argument_and_must_stay_inside_the_repo() {
        let d = Dirs::new("openfile");
        std::fs::write(d.0.join("fakeide"), "").unwrap();
        let repo = d.0.join("repo");
        std::fs::create_dir_all(repo.join("src dir")).unwrap();
        std::fs::write(repo.join("src dir").join("a b; rm -rf.ts"), "x").unwrap();
        let dirs = std::slice::from_ref(&d.0);

        let plan = plan_open_file(
            "fakeide --reuse-window",
            &repo,
            "src dir/a b; rm -rf.ts",
            dirs,
            &[],
        )
        .unwrap();
        assert_eq!(
            plan.args,
            vec![
                OsString::from("--reuse-window"),
                repo.join("src dir/a b; rm -rf.ts").as_os_str().to_owned()
            ]
        );
        for bad in ["", "../fakeide", "/etc/passwd", "src dir/../../fakeide"] {
            assert!(
                plan_open_file("fakeide", &repo, bad, dirs, &[]).is_err(),
                "{bad:?}"
            );
        }
        let gone = plan_open_file("fakeide", &repo, "src dir/deleted.ts", dirs, &[]).unwrap_err();
        assert!(gone.contains("doesn't exist"), "{gone}");
    }

    #[test]
    fn batch_launchers_refuse_cmd_metacharacters_in_the_folder() {
        let d = Dirs::new("batch");
        std::fs::write(d.0.join("code.cmd"), "").unwrap();
        let exts = vec![".cmd".to_string()];
        // `%` is legal in folder names on every platform.
        let risky = d.0.join("100%PATH%");
        std::fs::create_dir_all(&risky).unwrap();
        let err = plan_launch("code", &risky, std::slice::from_ref(&d.0), &exts).unwrap_err();
        assert!(err.contains(".exe"), "{err}");
        // A harmless name through the same shim is fine.
        let fine = d.0.join("fine");
        std::fs::create_dir_all(&fine).unwrap();
        assert!(plan_launch("code", &fine, std::slice::from_ref(&d.0), &exts).is_ok());
    }
}
