//! Patch-ids: a fingerprint of what a commit changes, independent of its
//! parent, date and message. A rebase, reword or reorder gives a commit a new
//! hash but the same patch-id, so a task's commit link can follow it.

use std::collections::HashMap;
use std::path::Path;

use git2::{Oid, Repository, Sort};

use crate::GitError;

/// How many commits per branch tip are searched for a rewritten commit.
const SEARCH_LIMIT: usize = 1000;

fn patch_id_in(repo: &Repository, oid: Oid) -> Result<Option<String>, GitError> {
    let commit = repo.find_commit(oid)?;
    // Merges have no single patch: they can't be re-matched.
    if commit.parent_count() > 1 {
        return Ok(None);
    }
    let tree = commit.tree()?;
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    if diff.deltas().len() == 0 {
        return Ok(None);
    }
    Ok(Some(diff.patchid(None)?.to_string()))
}

/// The patch-id of `hash`, or `None` for a merge, an empty commit or a commit
/// the repo doesn't have.
pub fn patch_id(path: &Path, hash: &str) -> Result<Option<String>, GitError> {
    let repo = Repository::open(path)?;
    let Ok(oid) = Oid::from_str(hash) else {
        return Ok(None);
    };
    if repo.find_commit(oid).is_err() {
        return Ok(None);
    }
    patch_id_in(&repo, oid)
}

/// A commit link's old hash and where it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewritten {
    pub old_hash: String,
    /// The commit with the same patch-id now on a branch and its patch-id, or
    /// `None` when no match was found (squashed away, dropped or edited).
    pub found: Option<(String, String)>,
}

/// For each old commit that is no longer on any branch, finds the commit that
/// now has the same patch-id. Commits still reachable from a branch are left
/// out of the answer. Old commits stay in the object database after a rebase
/// until `git gc`, which is what makes their patch-ids readable.
pub fn find_rewritten(path: &Path, old_hashes: &[String]) -> Result<Vec<Rewritten>, GitError> {
    let repo = Repository::open(path)?;
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TIME)?;
    let mut tips = 0;
    for pattern in ["refs/heads/*", "refs/remotes/*"] {
        if walk.push_glob(pattern).is_ok() {
            tips += 1;
        }
    }
    if tips == 0 {
        let _ = walk.push_head();
    }
    let mut reachable = std::collections::HashSet::new();
    let mut by_patch: HashMap<String, Oid> = HashMap::new();
    for oid in walk.take(SEARCH_LIMIT * 4).flatten() {
        reachable.insert(oid);
        if let Ok(Some(id)) = patch_id_in(&repo, oid) {
            by_patch.entry(id).or_insert(oid);
        }
    }
    let mut out = Vec::new();
    for old in old_hashes {
        let Ok(oid) = Oid::from_str(old) else {
            out.push(Rewritten {
                old_hash: old.clone(),
                found: None,
            });
            continue;
        };
        if reachable.contains(&oid) {
            continue;
        }
        let found = patch_id_in(&repo, oid)
            .ok()
            .flatten()
            .and_then(|id| by_patch.get(&id).map(|n| (n.to_string(), id)));
        out.push(Rewritten {
            old_hash: old.clone(),
            found,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::git_executable;
    use crate::rebase::{rebase_candidates, rebase_interactive, RebaseAction, RebaseEntry};
    use crate::testutil::Fixture;

    fn history(names: &[&str]) -> (Fixture, Vec<String>) {
        let f = Fixture::with_files(&[("base.txt", "base\n")]);
        let hashes = names
            .iter()
            .map(|n| {
                f.write(&format!("{n}.txt"), &format!("{n}\n"));
                f.stage_all();
                f.commit(n).to_string()
            })
            .collect();
        (f, hashes)
    }

    #[test]
    fn the_same_change_has_the_same_patch_id_whatever_its_parent() {
        let (f, h) = history(&["one", "two"]);
        let a = patch_id(f.path(), &h[0]).unwrap().unwrap();
        let b = patch_id(f.path(), &h[1]).unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(patch_id(f.path(), "nothex").unwrap(), None);
        assert_eq!(patch_id(f.path(), &"0".repeat(40)).unwrap(), None);
    }

    #[test]
    fn a_reworded_and_reordered_commit_is_found_again() {
        if git_executable().is_none() {
            return;
        }
        let (f, h) = history(&["one", "two", "three"]);
        // Reword "one" and put "three" first: every hash changes.
        let plan = vec![
            RebaseEntry {
                hash: h[2].clone(),
                action: RebaseAction::Pick,
            },
            RebaseEntry {
                hash: h[0].clone(),
                action: RebaseAction::Reword("first, renamed".into()),
            },
            RebaseEntry {
                hash: h[1].clone(),
                action: RebaseAction::Pick,
            },
        ];
        rebase_interactive(f.path(), &plan).unwrap();
        let now: Vec<String> = rebase_candidates(f.path(), 10)
            .unwrap()
            .into_iter()
            .map(|c| c.commit.hash)
            .collect();
        assert!(
            h.iter().all(|old| !now.contains(old)),
            "every commit was replayed"
        );

        let found = find_rewritten(f.path(), &h).unwrap();
        assert_eq!(found.len(), 3);
        for r in &found {
            let (new, pid) = r.found.as_ref().expect("matched by patch-id");
            assert!(now.contains(new));
            assert_eq!(
                patch_id(f.path(), new).unwrap().as_deref(),
                Some(pid.as_str())
            );
        }
    }

    #[test]
    fn squashed_away_commits_have_no_match_and_live_ones_are_skipped() {
        if git_executable().is_none() {
            return;
        }
        let (f, h) = history(&["one", "two", "three"]);
        let plan = vec![
            RebaseEntry {
                hash: h[0].clone(),
                action: RebaseAction::Pick,
            },
            RebaseEntry {
                hash: h[1].clone(),
                action: RebaseAction::Squash,
            },
            RebaseEntry {
                hash: h[2].clone(),
                action: RebaseAction::Pick,
            },
        ];
        rebase_interactive(f.path(), &plan).unwrap();
        let found = find_rewritten(f.path(), &h).unwrap();
        let by_old = |old: &String| found.iter().find(|r| &r.old_hash == old).unwrap();
        assert!(
            by_old(&h[0]).found.is_none(),
            "the squash changed its patch"
        );
        assert!(by_old(&h[1]).found.is_none());
        // The initial commit is untouched, so it would not be listed at all.
        let initial = Repository::open(f.path())
            .unwrap()
            .revparse_single("HEAD~2")
            .unwrap()
            .id()
            .to_string();
        assert!(find_rewritten(f.path(), &[initial]).unwrap().is_empty());
    }
}
