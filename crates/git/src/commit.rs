//! Committing what is staged, including amend and co-author trailers.
//!
//! libgit2 does not run git hooks (pre-commit, commit-msg), unlike the git
//! CLI. That is a known difference from GitHub Desktop; see docs/progress.md.

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
}
