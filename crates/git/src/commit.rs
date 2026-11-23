//! Committing what is staged, including amend and co-author trailers.
//!
//! libgit2 does not run git hooks (pre-commit, commit-msg), so when the repo
//! has an active hook and `git` is installed the commit goes through the git
//! CLI instead, as in GitHub Desktop. Without hooks libgit2 does it directly.

use std::path::Path;

use git2::{Repository, RepositoryState, Signature};

use crate::{CommitInfo, GitError};

#[derive(Debug, Clone, Copy)]
pub struct CommitRequest<'a> {
    /// First line of the message. Required.
    pub summary: &'a str,
    pub description: &'a str,
    /// `Name <email>` entries, added as `Co-authored-by:` trailers.
    pub co_authors: &'a [String],
    /// Replace the last commit with the staged state and this message.
    pub amend: bool,
}

fn invalid(msg: &str) -> GitError {
    GitError::Invalid(msg.to_owned())
}

/// A `Co-authored-by` value must look like `Name <email>` and stay on one line.
fn check_co_author(entry: &str) -> Result<&str, GitError> {
    let entry = entry.trim();
    let ok = !entry.contains(['\n', '\r'])
        && entry
            .find('<')
            .zip(entry.rfind('>'))
            .map(|(open, close)| open > 0 && close == entry.len() - 1 && close > open + 1)
            .unwrap_or(false);
    if ok {
        Ok(entry)
    } else {
        Err(GitError::Invalid(format!(
            "co-author must look like `Name <email>`: {entry}"
        )))
    }
}

fn build_message(req: &CommitRequest<'_>) -> Result<String, GitError> {
    let summary = req.summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return Err(invalid("write a summary for the commit"));
    }
    let mut msg = summary;
    let description = req.description.trim();
    if !description.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(description);
    }
    let mut trailers = Vec::new();
    for entry in req.co_authors {
        let entry = check_co_author(entry)?;
        // The user may already have typed the trailer into the description.
        if !msg.contains(&format!("Co-authored-by: {entry}")) {
            trailers.push(format!("Co-authored-by: {entry}"));
        }
    }
    if !trailers.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&trailers.join("\n"));
    }
    msg.push('\n');
    Ok(msg)
}

fn commit_info(commit: &git2::Commit<'_>) -> CommitInfo {
    CommitInfo {
        hash: commit.id().to_string(),
        patch_id: commit.tree_id().to_string(),
        summary: commit.summary().unwrap_or("").to_owned(),
        author: commit.author().name().unwrap_or("").to_owned(),
        time: commit.time().seconds(),
    }
}

fn signature(repo: &Repository) -> Result<Signature<'static>, GitError> {
    repo.signature().map_err(|_| {
        invalid("git doesn't know who you are: set user.name and user.email (git config --global)")
    })
}

/// Commits the index. Fails with a readable message when nothing is staged,
/// conflicts are unresolved, or a merge/rebase is half done (finish or abort
/// it first).
pub fn commit_staged(path: &Path, req: &CommitRequest<'_>) -> Result<CommitInfo, GitError> {
    let message = build_message(req)?;
    let repo = Repository::open(path)?;
    if repo.state() != RepositoryState::Clean {
        return Err(invalid(
            "finish or abort the merge, rebase or cherry-pick in progress first",
        ));
    }
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(invalid("resolve the conflicted files first"));
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let sig = signature(&repo)?;

    if has_active_hooks(&repo) && crate::git_executable().is_some() {
        if !req.amend {
            let unchanged = match &head {
                Some(h) => h.tree_id() == tree.id(),
                None => tree.is_empty(),
            };
            if unchanged {
                return Err(invalid("nothing is staged"));
            }
        } else if head.is_none() {
            return Err(invalid("there is no commit to amend"));
        }
        drop((tree, index));
        return commit_with_git(path, &repo, &message, req.amend);
    }

    let oid = if req.amend {
        let head = head.ok_or_else(|| invalid("there is no commit to amend"))?;
        head.amend(
            Some("HEAD"),
            None,
            Some(&sig),
            None,
            Some(&message),
            Some(&tree),
        )?
    } else {
        let unchanged = match &head {
            Some(h) => h.tree_id() == tree.id(),
            None => tree.is_empty(),
        };
        if unchanged {
            return Err(invalid("nothing is staged"));
        }
        let parents: Vec<&git2::Commit<'_>> = head.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)?
    };
    let commit = repo.find_commit(oid)?;
    let info = commit_info(&commit);
    Ok(info)
}

/// Whether the repo has a hook that a commit would run.
fn has_active_hooks(repo: &Repository) -> bool {
    const HOOKS: [&str; 4] = [
        "pre-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
    ];
    let dir = match repo.config().and_then(|c| c.get_path("core.hooksPath")) {
        Ok(p) if p.is_absolute() => p,
        Ok(p) => repo.workdir().unwrap_or_else(|| repo.path()).join(p),
        Err(_) => repo.path().join("hooks"),
    };
    HOOKS.iter().any(|name| {
        let hook = dir.join(name);
        hook.is_file() || hook.with_extension("exe").is_file()
    })
}

/// Commits through the git CLI so hooks run. The message goes in a file, never
/// on the command line, and a hook that rejects the commit explains itself in
/// the returned error.
fn commit_with_git(
    path: &Path,
    repo: &Repository,
    message: &str,
    amend: bool,
) -> Result<CommitInfo, GitError> {
    let file = repo.path().join("devspace-commit-msg.txt");
    std::fs::write(&file, message)?;
    let file_str = file.to_string_lossy().into_owned();
    let mut args = vec!["commit", "-F", file_str.as_str()];
    if amend {
        args.push("--amend");
    }
    let result = crate::cli::run_ok(path, &args, &[], crate::cli::Kind::Local);
    let _ = std::fs::remove_file(&file);
    result?;
    let repo = Repository::open(path)?;
    let commit = repo.head()?.peel_to_commit()?;
    Ok(commit_info(&commit))
}

/// Summary and description of HEAD's commit, to pre-fill the box when
/// amending. `None` when there are no commits.
pub fn last_commit_message(path: &Path) -> Result<Option<(String, String)>, GitError> {
    let repo = Repository::open(path)?;
    let Some(commit) = repo.head().ok().and_then(|h| h.peel_to_commit().ok()) else {
        return Ok(None);
    };
    let summary = commit.summary().unwrap_or("").to_owned();
    let description = commit
        .message()
        .unwrap_or("")
        .split_once('\n')
        .map(|(_, rest)| rest.trim().to_owned())
        .unwrap_or_default();
    Ok(Some((summary, description)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;
    use crate::{repo_status, stage_files};

    fn req<'a>(summary: &'a str) -> CommitRequest<'a> {
        CommitRequest {
            summary,
            description: "",
            co_authors: &[],
            amend: false,
        }
    }

    fn head_message(f: &Fixture) -> String {
        f.repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn commits_only_what_is_staged() {
        let f = Fixture::with_files(&[("a.txt", "1\n"), ("b.txt", "1\n")]);
        f.write("a.txt", "2\n");
        f.write("b.txt", "2\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let info = commit_staged(f.path(), &req("Update a")).unwrap();
        assert_eq!(info.summary, "Update a");
        let st = repo_status(f.path()).unwrap();
        assert_eq!(st.files.len(), 1);
        assert_eq!(st.files[0].path, "b.txt");
    }

    #[test]
    fn first_commit_in_an_empty_repo() {
        let f = Fixture::empty();
        f.write("a.txt", "1\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        commit_staged(f.path(), &req("Initial")).unwrap();
        assert!(!repo_status(f.path()).unwrap().unborn);
    }

    #[test]
    fn nothing_staged_is_an_error() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        let err = commit_staged(f.path(), &req("x")).unwrap_err();
        assert!(err.to_string().contains("nothing is staged"));
        let empty = Fixture::empty();
        assert!(commit_staged(empty.path(), &req("x")).is_err());
    }

    #[test]
    fn empty_summary_is_refused() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        assert!(commit_staged(f.path(), &req("   ")).is_err());
    }

    #[test]
    fn message_has_description_and_co_author_trailers() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let co = vec!["Ann Lee <ann@example.com>".to_owned()];
        commit_staged(
            f.path(),
            &CommitRequest {
                summary: "Fix\nthe thing",
                description: "More detail.\n",
                co_authors: &co,
                amend: false,
            },
        )
        .unwrap();
        assert_eq!(
            head_message(&f),
            "Fix the thing\n\nMore detail.\n\nCo-authored-by: Ann Lee <ann@example.com>\n"
        );
    }

    #[test]
    fn bad_co_authors_are_refused() {
        for bad in [
            "Ann",
            "<a@b.c>",
            "Ann <a@b.c> trailing",
            "Ann <a@b.c>\nInjected: x",
        ] {
            let r = build_message(&CommitRequest {
                summary: "s",
                description: "",
                co_authors: &[bad.to_owned()],
                amend: false,
            });
            assert!(r.is_err(), "{bad:?}");
        }
    }

    #[test]
    fn amend_replaces_the_last_commit() {
        let f = Fixture::with_files(&[("a.txt", "1\n"), ("b.txt", "1\n")]);
        f.write("a.txt", "2\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        commit_staged(f.path(), &req("First try")).unwrap();
        let before = f
            .repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_id(0)
            .unwrap();

        f.write("b.txt", "2\n");
        stage_files(f.path(), &["b.txt".into()]).unwrap();
        commit_staged(
            f.path(),
            &CommitRequest {
                amend: true,
                ..req("Better message")
            },
        )
        .unwrap();
        let head = f.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(
            head.parent_id(0).unwrap(),
            before,
            "still one commit on top"
        );
        assert_eq!(head.summary(), Some("Better message"));
        assert!(repo_status(f.path()).unwrap().files.is_empty());
    }

    #[test]
    fn amend_can_change_only_the_message() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        commit_staged(
            f.path(),
            &CommitRequest {
                amend: true,
                ..req("Reworded")
            },
        )
        .unwrap();
        assert_eq!(head_message(&f), "Reworded\n");
    }

    #[test]
    fn last_message_splits_summary_and_description() {
        let f = Fixture::with_files(&[("a.txt", "1\n")]);
        f.write("a.txt", "2\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        commit_staged(
            f.path(),
            &CommitRequest {
                description: "body text",
                ..req("Title")
            },
        )
        .unwrap();
        assert_eq!(
            last_commit_message(f.path()).unwrap(),
            Some(("Title".to_owned(), "body text".to_owned()))
        );
        assert_eq!(last_commit_message(Fixture::empty().path()).unwrap(), None);
    }

    #[test]
    fn unresolved_conflicts_block_committing() {
        let f = Fixture::with_files(&[("a.txt", "base\n")]);
        let base = f.branch_name().unwrap();
        f.make_branch("other");
        f.write("a.txt", "main side\n");
        f.stage_all();
        f.commit("main change");
        f.repo.set_head("refs/heads/other").unwrap();
        f.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        f.write("a.txt", "other side\n");
        f.stage_all();
        f.commit("other change");
        let theirs = f
            .repo
            .find_annotated_commit(
                f.repo
                    .find_branch(&base, git2::BranchType::Local)
                    .unwrap()
                    .get()
                    .peel_to_commit()
                    .unwrap()
                    .id(),
            )
            .unwrap();
        f.repo.merge(&[&theirs], None, None).unwrap();
        assert_eq!(repo_status(f.path()).unwrap().in_progress, Some("merge"));
        assert!(commit_staged(f.path(), &req("merge")).is_err());
    }

    fn write_hook(f: &Fixture, name: &str, body: &str) {
        let dir = f.path().join(".git").join("hooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(name),
            format!(
                "#!/bin/sh
{body}
"
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_failing_pre_commit_hook_blocks_the_commit_and_explains_why() {
        if crate::git_executable().is_none() {
            return;
        }
        let f = Fixture::with_files(&[(
            "a.txt", "one
",
        )]);
        write_hook(
            &f,
            "pre-commit",
            "echo \"lint failed\" >&2
exit 1",
        );
        f.write(
            "a.txt", "two
",
        );
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let err = commit_staged(f.path(), &req("change"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("lint failed"), "{err}");
        assert_eq!(
            repo_status(f.path()).unwrap().staged_count(),
            1,
            "still staged"
        );
    }

    #[test]
    fn hooks_run_and_the_commit_still_lands_with_trailers() {
        if crate::git_executable().is_none() {
            return;
        }
        let f = Fixture::with_files(&[(
            "a.txt", "one
",
        )]);
        write_hook(&f, "commit-msg", "exit 0");
        f.write(
            "a.txt", "two
",
        );
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let co = ["Pat <pat@example.com>".to_string()];
        let r = CommitRequest {
            co_authors: &co,
            ..req("change")
        };
        let info = commit_staged(f.path(), &r).unwrap();
        assert_eq!(info.summary, "change");
        let (_, body) = last_commit_message(f.path()).unwrap().unwrap();
        assert!(body.contains("Co-authored-by: Pat"), "{body}");
        assert_eq!(repo_status(f.path()).unwrap().staged_count(), 0);
    }
}
