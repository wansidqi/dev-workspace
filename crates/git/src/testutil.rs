//! Fixtures for the working-copy client tests. Built with git2 only, so they
//! run without a git executable.

use std::path::Path;

use git2::{Repository, Signature};
use tempfile::TempDir;

pub struct Fixture {
    pub dir: TempDir,
    pub repo: Repository,
}

impl Fixture {
    /// A repo with identity configured and no commits.
    pub fn empty() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@example.com").unwrap();
            // Keep fixtures byte-exact on every platform.
            cfg.set_str("core.autocrlf", "false").unwrap();
        }
        Self { dir, repo }
    }

    /// A repo whose first commit holds `files` (path, content).
    pub fn with_files(files: &[(&str, &str)]) -> Self {
        let f = Self::empty();
        for (path, content) in files {
            f.write(path, content);
        }
        f.stage_all();
        f.commit("initial");
        f
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write(&self, rel: &str, content: &str) {
        let full = self.dir.path().join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.dir.path().join(rel).exists()
    }

    /// A handle that reads the index from disk: `self.repo` caches its own
    /// copy, which goes stale when the code under test writes the index.
    fn fresh(&self) -> Repository {
        Repository::open(self.dir.path()).unwrap()
    }

    pub fn stage_all(&self) {
        let repo = self.fresh();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
    }

    pub fn commit(&self, message: &str) -> git2::Oid {
        let sig = Signature::now("Test", "test@example.com").unwrap();
        let repo = self.fresh();
        let mut index = repo.index().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    /// Blob content of `rel` in the index.
    pub fn index_content(&self, rel: &str) -> Option<String> {
        let repo = self.fresh();
        let index = repo.index().unwrap();
        let entry = index.get_path(Path::new(rel), 0)?;
        let blob = repo.find_blob(entry.id).unwrap();
        Some(String::from_utf8_lossy(blob.content()).into_owned())
    }

    pub fn branch_name(&self) -> Option<String> {
        self.repo
            .head()
            .ok()
            .and_then(|h| h.shorthand().map(str::to_owned))
    }

    pub fn make_branch(&self, name: &str) {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        self.repo.branch(name, &head, false).unwrap();
    }
}
