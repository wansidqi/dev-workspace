//! Reading a month of one author's commits for the timesheet.

use std::path::Path;

use devspace_core::{date_from_unix, TimesheetCommit};
use git2::{Commit, Patch, Repository, Sort};

use crate::GitError;

/// Commits of the repo at `path` whose author date falls in `from..=to`
/// (`YYYY-MM-DD`, in the author's own timezone, so the day matches what they
/// saw when committing). Walks every local and remote branch, skips merge
/// commits, and keeps only commits whose author name or email contains one
/// of `authors` (case-insensitive); an empty list keeps everyone.
///
/// Each commit carries its changed-line count with lockfiles and generated
/// output left out, so they do not inflate the hours estimate. Commits come
/// back newest first and can repeat across repos; the caller groups them.
pub fn timesheet_commits(
    path: &Path,
    repo_name: &str,
    from: &str,
    to: &str,
    authors: &[String],
) -> Result<Vec<TimesheetCommit>, GitError> {
    let repo = Repository::open(path)?;
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TIME)?;
    // `git log --all` minus tags and stashes. An unborn HEAD or a pattern
    // with no match is not an error: the walk is just empty.
    let _ = walk.push_head();
    for glob in ["refs/heads/*", "refs/remotes/*"] {
        walk.push_glob(glob)?;
    }
    let authors: Vec<String> = authors.iter().map(|a| a.to_lowercase()).collect();

    let mut out = Vec::new();
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        if commit.parent_count() > 1 {
            continue;
        }
        let author = commit.author();
        let when = author.when();
        let date = date_from_unix(when.seconds() + i64::from(when.offset_minutes()) * 60);
        if date.as_str() < from || date.as_str() > to {
            continue;
        }
        let who = format!(
            "{} <{}>",
            author.name().unwrap_or_default(),
            author.email().unwrap_or_default()
        )
        .to_lowercase();
        if !authors.is_empty() && !authors.iter().any(|a| who.contains(a.as_str())) {
            continue;
        }
        out.push(TimesheetCommit {
            repo: repo_name.to_string(),
            hash: commit.id().to_string(),
            subject: commit.summary().unwrap_or_default().to_string(),
            date,
            // A commit whose diff cannot be read still counts, with no lines,
            // rather than failing the whole month.
            lines: changed_lines(&repo, &commit).unwrap_or(0),
        });
    }
    Ok(out)
}

/// Added plus deleted lines against the first parent (or the empty tree for
/// a root commit), renames detected, noise files and binaries excluded.
fn changed_lines(repo: &Repository, commit: &Commit) -> Result<u32, git2::Error> {
    let tree = commit.tree()?;
    let parent_tree = match commit.parent_count() {
        0 => None,
        _ => Some(commit.parent(0)?.tree()?),
    };
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    diff.find_similar(None)?;

    let mut lines = 0usize;
    for index in 0..diff.deltas().len() {
        let noise = diff
            .get_delta(index)
            .and_then(|d| d.new_file().path().map(|p| is_noise(&p.to_string_lossy())))
            .unwrap_or(false);
        if noise {
            continue;
        }
        if let Some(patch) = Patch::from_diff(&diff, index)? {
            let (_context, added, deleted) = patch.line_stats()?;
            lines += added + deleted;
        }
    }
    Ok(u32::try_from(lines).unwrap_or(u32::MAX))
}

/// Lockfiles and generated output: `dist/`, `build/` or `.next/` at any
/// depth, the common lockfiles, and `.min.js`, `.map`, `.snap`, `.lock`.
fn is_noise(path: &str) -> bool {
    let path = path.replace('\\', "/");
    let name = path.rsplit('/').next().unwrap_or_default();
    if matches!(
        name,
        "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "composer.lock"
    ) {
        return true;
    }
    if [".min.js", ".map", ".snap", ".lock"]
        .iter()
        .any(|ext| name.ends_with(ext))
    {
        return true;
    }
    path.split('/')
        .rev()
        .skip(1)
        .any(|dir| matches!(dir, "dist" | "build" | ".next"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    /// 2026-10-01 00:00 UTC.
    const OCT_1: i64 = 1_790_812_800;
    const DAY: i64 = 86_400;

    fn git(dir: &Path, args: &[&str], time: Option<i64>) {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        if let Some(t) = time {
            let date = format!("@{t} +0000");
            cmd.env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date);
        }
        assert!(cmd.status().expect("git must be installed").success());
    }

    fn init() -> TempDir {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"], None);
        git(
            dir.path(),
            &["config", "user.email", "ann@example.com"],
            None,
        );
        git(dir.path(), &["config", "user.name", "Ann"], None);
        dir
    }

    /// Commits `files` (path, content) at `time`, optionally as someone else.
    fn commit(dir: &Path, message: &str, time: i64, files: &[(&str, &str)], author: Option<&str>) {
        for (name, content) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        git(dir, &["add", "."], None);
        let mut args = vec!["commit", "-q", "-m", message];
        let author_arg;
        if let Some(a) = author {
            author_arg = format!("--author={a}");
            args.push(&author_arg);
        }
        git(dir, &args, Some(time));
    }

    fn read(dir: &TempDir, from: &str, to: &str, authors: &[&str]) -> Vec<TimesheetCommit> {
        let authors: Vec<String> = authors.iter().map(|a| a.to_string()).collect();
        timesheet_commits(dir.path(), "app", from, to, &authors).unwrap()
    }

    #[test]
    fn keeps_only_the_month_and_the_author() {
        let dir = init();
        commit(dir.path(), "old", OCT_1 - DAY / 2, &[("a.txt", "a")], None);
        commit(
            dir.path(),
            "7267: mine",
            OCT_1 + DAY,
            &[("b.txt", "b")],
            None,
        );
        commit(
            dir.path(),
            "theirs",
            OCT_1 + 2 * DAY,
            &[("c.txt", "c")],
            Some("Bob <bob@example.com>"),
        );
        commit(
            dir.path(),
            "late",
            OCT_1 + 31 * DAY,
            &[("d.txt", "d")],
            None,
        );

        let mine = read(&dir, "2026-10-01", "2026-10-31", &["ann"]);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].subject, "7267: mine");
        assert_eq!(mine[0].date, "2026-10-02");
        assert_eq!(mine[0].repo, "app");

        let everyone = read(&dir, "2026-10-01", "2026-10-31", &[]);
        assert_eq!(everyone.len(), 2);
        // The filter matches the email too, ignoring case.
        let bob = read(&dir, "2026-10-01", "2026-10-31", &["BOB@example"]);
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].subject, "theirs");
    }

    #[test]
    fn counts_lines_without_lockfiles_and_generated_output() {
        let dir = init();
        commit(dir.path(), "base", OCT_1, &[("base.txt", "x\n")], None);
        commit(
            dir.path(),
            "change",
            OCT_1 + DAY,
            &[
                ("src.txt", "1\n2\n3\n"),
                ("package-lock.json", "{}\n{}\n{}\n{}\n"),
                ("dist/out.js", "a\nb\nc\n"),
                ("app.min.js", "m\n"),
            ],
            None,
        );
        let commits = read(&dir, "2026-10-01", "2026-10-31", &[]);
        let change = commits.iter().find(|c| c.subject == "change").unwrap();
        assert_eq!(change.lines, 3);
        let base = commits.iter().find(|c| c.subject == "base").unwrap();
        assert_eq!(base.lines, 1, "a root commit diffs against the empty tree");
    }

    #[test]
    fn walks_other_branches_and_skips_merges() {
        let dir = init();
        commit(dir.path(), "on main", OCT_1, &[("m.txt", "m")], None);
        git(dir.path(), &["checkout", "-q", "-b", "feature"], None);
        commit(
            dir.path(),
            "on feature",
            OCT_1 + DAY,
            &[("f.txt", "f")],
            None,
        );
        git(dir.path(), &["checkout", "-q", "main"], None);
        commit(
            dir.path(),
            "more main",
            OCT_1 + 2 * DAY,
            &[("n.txt", "n")],
            None,
        );
        git(
            dir.path(),
            &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
            Some(OCT_1 + 3 * DAY),
        );
        git(dir.path(), &["checkout", "-q", "--detach", "HEAD~1"], None);

        let subjects: Vec<String> = read(&dir, "2026-10-01", "2026-10-31", &[])
            .into_iter()
            .map(|c| c.subject)
            .collect();
        assert!(subjects.contains(&"on feature".to_string()));
        assert!(!subjects.contains(&"merge feature".to_string()));
        assert_eq!(subjects.len(), 3);
    }

    #[test]
    fn an_empty_repo_has_no_commits() {
        let dir = init();
        assert!(read(&dir, "2026-10-01", "2026-10-31", &[]).is_empty());
    }

    #[test]
    fn noise_paths() {
        for noise in [
            "package-lock.json",
            "sub/yarn.lock",
            "a/dist/x.js",
            "build/y.css",
            ".next/z",
            "x.min.js",
            "bundle.js.map",
            "__snapshots__/a.snap",
            "win\\dist\\x.js",
        ] {
            assert!(is_noise(noise), "{noise}");
        }
        for real in ["src/main.rs", "distance.txt", "dist", "docs/build.md"] {
            assert!(!is_noise(real), "{real}");
        }
    }
}
