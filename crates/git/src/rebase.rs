//! Interactive rebase (Phase 4): squash, fixup, reorder, reword and drop the
//! commits at the tip of the current branch.
//!
//! The todo list is built here and handed to `git rebase -i` through
//! `GIT_SEQUENCE_EDITOR`, so no editor ever opens. Rewritten messages are
//! written to files and applied with an `exec git commit --amend -F` step.

use std::path::{Path, PathBuf};

use git2::Repository;

use crate::cli::{is_full_hash, run, Kind};
use crate::{repo_status, CommitInfo, GitError};

fn invalid(msg: impl Into<String>) -> GitError {
    GitError::Invalid(msg.into())
}

/// What to do with one commit of the rebase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseAction {
    /// Keep the commit as it is.
    Pick,
    /// Keep the commit with a new message (for a squash group, the message of
    /// the whole group).
    Reword(String),
    /// Fold into the commit before it, joining the messages.
    Squash,
    /// Fold into the commit before it, dropping this message.
    Fixup,
    /// Remove the commit.
    Drop,
}

/// One line of the plan, in the order it will be applied (oldest first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseEntry {
    pub hash: String,
    pub action: RebaseAction,
}

/// A commit offered for rewriting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseCandidate {
    pub commit: CommitInfo,
    /// Not on the upstream yet: rewriting it needs no force push.
    pub unpushed: bool,
}

/// The newest `limit` commits of the current branch that can be rewritten,
/// newest first. The list stops before a merge commit or the root, since a
/// rebase needs a plain run of commits with a parent to start from.
pub fn rebase_candidates(path: &Path, limit: usize) -> Result<Vec<RebaseCandidate>, GitError> {
    let repo = Repository::open(path)?;
    let head = repo
        .head()
        .map_err(|_| invalid("the branch has no commits yet"))?
        .peel_to_commit()?;
    let status = repo_status(path)?;
    let unpushed_count = if status.upstream.is_some() {
        status.ahead
    } else {
        usize::MAX
    };
    let mut out = Vec::new();
    let mut cursor = Some(head);
    while let Some(commit) = cursor {
        if out.len() >= limit || commit.parent_count() != 1 {
            break;
        }
        out.push(RebaseCandidate {
            commit: CommitInfo {
                hash: commit.id().to_string(),
                patch_id: commit.tree_id().to_string(),
                summary: commit.summary().unwrap_or("").to_owned(),
                author: commit.author().name().unwrap_or("").to_owned(),
                time: commit.time().seconds(),
            },
            unpushed: out.len() < unpushed_count,
        });
        cursor = commit.parent(0).ok();
    }
    Ok(out)
}

/// Checks a plan before it touches the repo: full hashes, no repeats, a
/// squash or fixup always has a commit before it, and something is kept.
pub fn validate_plan(entries: &[RebaseEntry]) -> Result<(), GitError> {
    if entries.is_empty() {
        return Err(invalid("pick at least one commit"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut kept_before = false;
    for entry in entries {
        if !is_full_hash(&entry.hash) {
            return Err(invalid("not a commit id"));
        }
        if !seen.insert(entry.hash.as_str()) {
            return Err(invalid("a commit appears twice in the plan"));
        }
        match &entry.action {
            RebaseAction::Squash | RebaseAction::Fixup if !kept_before => {
                return Err(invalid(
                    "the first commit can't be squashed: there is nothing before it to join",
                ));
            }
            RebaseAction::Reword(message) if message.trim().is_empty() => {
                return Err(invalid("a commit message can't be empty"));
            }
            RebaseAction::Drop => {}
            _ => kept_before = true,
        }
    }
    if !kept_before {
        return Err(invalid("the plan drops every commit"));
    }
    Ok(())
}

/// Single-quotes `text` for the shell git runs the sequence editor with.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Forward slashes work in Git for Windows' shell; backslashes would be eaten.
fn sh_path(path: &Path) -> String {
    sh_quote(&path.to_string_lossy().replace('\\', "/"))
}

/// Builds the todo text and the message files it refers to. `message_file`
/// maps a message to the path it was (or will be) written to.
pub(crate) fn build_todo(
    entries: &[RebaseEntry],
    mut message_file: impl FnMut(&str) -> PathBuf,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    // The new message for the group being built, applied after its last line.
    let mut pending_message: Option<String> = None;
    let flush = |lines: &mut Vec<String>,
                 pending: &mut Option<String>,
                 files: &mut dyn FnMut(&str) -> PathBuf| {
        if let Some(message) = pending.take() {
            let file = files(&message);
            lines.push(format!(
                "exec git commit --amend --no-verify -q -F {}",
                sh_path(&file)
            ));
        }
    };
    for entry in entries {
        match &entry.action {
            RebaseAction::Drop => lines.push(format!("drop {}", entry.hash)),
            RebaseAction::Squash => lines.push(format!("squash {}", entry.hash)),
            RebaseAction::Fixup => lines.push(format!("fixup {}", entry.hash)),
            RebaseAction::Pick => {
                flush(&mut lines, &mut pending_message, &mut message_file);
                lines.push(format!("pick {}", entry.hash));
            }
            RebaseAction::Reword(message) => {
                flush(&mut lines, &mut pending_message, &mut message_file);
                lines.push(format!("pick {}", entry.hash));
                pending_message = Some(message.clone());
            }
        }
    }
    flush(&mut lines, &mut pending_message, &mut message_file);
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// How the rebase ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    Done,
    /// Stopped on a conflict; resolve and continue, or abort.
    Conflicts(usize),
}

/// Rewrites the tip of the current branch as `entries` says (oldest first).
/// The entries must be exactly the newest run of commits as returned by
/// [`rebase_candidates`] (any prefix of that run counts as long as the
/// oldest entry's parent is the base). Needs a clean working copy.
pub fn rebase_interactive(path: &Path, entries: &[RebaseEntry]) -> Result<RebaseOutcome, GitError> {
    validate_plan(entries)?;
    let repo = Repository::open(path)?;
    if repo.state() != git2::RepositoryState::Clean {
        return Err(invalid(
            "another operation is unfinished: continue or abort it first",
        ));
    }
    let status = repo_status(path)?;
    if !status.files.is_empty() {
        return Err(invalid(
            "commit, stash or discard your changes before rewriting history",
        ));
    }
    // The plan must be the run of commits ending at HEAD, so nothing else is replayed.
    let head = repo.head()?.peel_to_commit()?;
    let mut expected = Vec::new();
    let mut cursor = Some(head);
    while let Some(commit) = cursor {
        if expected.len() >= entries.len() {
            cursor = Some(commit);
            break;
        }
        if commit.parent_count() != 1 {
            return Err(invalid(
                "the commits to rewrite include a merge or the first commit",
            ));
        }
        expected.push(commit.id().to_string());
        cursor = commit.parent(0).ok();
    }
    let base = cursor
        .ok_or_else(|| invalid("the first commit can't be rewritten"))?
        .id()
        .to_string();
    let mut planned: Vec<&str> = entries.iter().map(|e| e.hash.as_str()).collect();
    planned.sort_unstable();
    expected.sort_unstable();
    if planned != expected.iter().map(String::as_str).collect::<Vec<_>>() {
        return Err(invalid(
            "the plan doesn't match the newest commits of this branch; reload and try again",
        ));
    }

    // Everything the editor hooks need lives in a scratch folder in .git.
    let dir = repo.path().join("devspace-rebase");
    std::fs::create_dir_all(&dir)?;
    let mut counter = 0usize;
    let todo = build_todo(entries, |message| {
        counter += 1;
        let file = dir.join(format!("message-{counter}.txt"));
        let _ = std::fs::write(&file, message);
        file
    });
    let todo_file = dir.join("todo.txt");
    std::fs::write(&todo_file, todo)?;
    let sequence_editor = format!("cp {}", sh_path(&todo_file));

    let out = run(
        path,
        &["rebase", "-i", "--no-autosquash", &base],
        &[
            ("GIT_SEQUENCE_EDITOR", &sequence_editor),
            ("GIT_EDITOR", "true"),
        ],
        Kind::Local,
    )?;
    let after = repo_status(path)?;
    if after.in_progress.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    if out.success {
        return Ok(RebaseOutcome::Done);
    }
    if after.in_progress == Some("rebase") {
        return Ok(RebaseOutcome::Conflicts(after.conflict_count()));
    }
    Err(invalid(out.message()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::git_executable;
    use crate::testutil::Fixture;

    fn hash(c: char) -> String {
        c.to_string().repeat(40)
    }

    fn entry(c: char, action: RebaseAction) -> RebaseEntry {
        RebaseEntry {
            hash: hash(c),
            action,
        }
    }

    #[test]
    fn todo_orders_lines_and_attaches_new_messages_to_their_group() {
        let plan = [
            entry('a', RebaseAction::Reword("New A".into())),
            entry('b', RebaseAction::Squash),
            entry('c', RebaseAction::Drop),
            entry('d', RebaseAction::Pick),
            entry('e', RebaseAction::Fixup),
        ];
        let mut files = Vec::new();
        let todo = build_todo(&plan, |m| {
            files.push(m.to_owned());
            PathBuf::from(format!("C:\\tmp\\m{}.txt", files.len()))
        });
        let lines: Vec<&str> = todo.lines().collect();
        assert_eq!(lines[0], format!("pick {}", hash('a')));
        assert_eq!(lines[1], format!("squash {}", hash('b')));
        assert_eq!(lines[2], format!("drop {}", hash('c')));
        assert!(
            lines[3].starts_with("exec git commit --amend")
                && lines[3].ends_with("'C:/tmp/m1.txt'"),
            "{}",
            lines[3]
        );
        assert_eq!(lines[4], format!("pick {}", hash('d')));
        assert_eq!(lines[5], format!("fixup {}", hash('e')));
        assert_eq!(files, ["New A"]);
    }

    #[test]
    fn plans_are_checked_before_anything_runs() {
        use RebaseAction::*;
        assert!(validate_plan(&[]).is_err());
        assert!(
            validate_plan(&[entry('a', Squash)]).is_err(),
            "nothing to squash into"
        );
        assert!(validate_plan(&[entry('a', Drop), entry('b', Fixup)]).is_err());
        assert!(
            validate_plan(&[entry('a', Drop)]).is_err(),
            "everything dropped"
        );
        assert!(validate_plan(&[entry('a', Pick), entry('a', Pick)]).is_err());
        assert!(validate_plan(&[entry('a', Reword("  ".into()))]).is_err());
        assert!(validate_plan(&[RebaseEntry {
            hash: "--exec=evil".into(),
            action: Pick
        }])
        .is_err());
        assert!(validate_plan(&[entry('a', Pick), entry('b', Squash)]).is_ok());
    }

    #[test]
    fn quoting_survives_apostrophes() {
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    fn history(messages: &[&str]) -> Fixture {
        let f = Fixture::with_files(&[("base.txt", "base\n")]);
        for m in messages {
            f.write(&format!("{m}.txt"), &format!("{m}\n"));
            f.stage_all();
            f.commit(m);
        }
        f
    }

    fn summaries(f: &Fixture) -> Vec<String> {
        let repo = Repository::open(f.path()).unwrap();
        let mut walk = repo.revwalk().unwrap();
        walk.push_head().unwrap();
        walk.map(|o| {
            repo.find_commit(o.unwrap())
                .unwrap()
                .summary()
                .unwrap()
                .to_owned()
        })
        .collect()
    }

    fn plan_for(f: &Fixture, n: usize) -> Vec<RebaseEntry> {
        let mut c = rebase_candidates(f.path(), n).unwrap();
        c.reverse();
        c.into_iter()
            .map(|c| RebaseEntry {
                hash: c.commit.hash,
                action: RebaseAction::Pick,
            })
            .collect()
    }

    #[test]
    fn candidates_stop_before_the_root_commit() {
        let f = history(&["one", "two"]);
        let c = rebase_candidates(f.path(), 10).unwrap();
        // initial commit is the root, so it can't be offered.
        assert_eq!(
            c.iter()
                .map(|c| c.commit.summary.as_str())
                .collect::<Vec<_>>(),
            ["two", "one"]
        );
        assert!(
            c.iter().all(|c| c.unpushed),
            "no upstream: nothing is published"
        );
    }

    #[test]
    fn squash_reword_reorder_and_drop_rewrite_history() {
        if git_executable().is_none() {
            return;
        }
        let f = history(&["one", "two", "three", "four"]);
        let mut plan = plan_for(&f, 4); // one, two, three, four
                                        // Order: three first, then one (reworded) with two squashed in, drop four.
        plan = vec![
            plan[2].clone(),
            RebaseEntry {
                action: RebaseAction::Reword("combined one and two".into()),
                ..plan[0].clone()
            },
            RebaseEntry {
                action: RebaseAction::Squash,
                ..plan[1].clone()
            },
            RebaseEntry {
                action: RebaseAction::Drop,
                ..plan[3].clone()
            },
        ];
        assert_eq!(
            rebase_interactive(f.path(), &plan).unwrap(),
            RebaseOutcome::Done
        );
        assert_eq!(summaries(&f), ["combined one and two", "three", "initial"]);
        assert!(f.exists("one.txt") && f.exists("two.txt") && f.exists("three.txt"));
        assert!(!f.exists("four.txt"));
        assert_eq!(repo_status(f.path()).unwrap().in_progress, None);
    }

    #[test]
    fn a_dirty_working_copy_or_a_stale_plan_is_refused() {
        if git_executable().is_none() {
            return;
        }
        let f = history(&["one", "two"]);
        let plan = plan_for(&f, 2);
        f.write("loose.txt", "x");
        assert!(rebase_interactive(f.path(), &plan)
            .unwrap_err()
            .to_string()
            .contains("stash"));
        std::fs::remove_file(f.path().join("loose.txt")).unwrap();
        // Only the older commit: it isn't the tip run.
        assert!(rebase_interactive(f.path(), &plan[..1]).is_err());
    }

    #[test]
    fn a_conflicting_reorder_stops_and_can_be_aborted() {
        if git_executable().is_none() {
            return;
        }
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        f.stage_all();
        f.commit("two");
        f.write("a.txt", "3\n");
        f.stage_all();
        f.commit("three");
        let mut plan = plan_for(&f, 2);
        plan.reverse();
        assert_eq!(
            rebase_interactive(f.path(), &plan).unwrap(),
            RebaseOutcome::Conflicts(1)
        );
        assert_eq!(repo_status(f.path()).unwrap().in_progress, Some("rebase"));
        crate::abort_operation(f.path()).unwrap();
        assert_eq!(summaries(&f), ["three", "two", "initial"]);
    }
}
