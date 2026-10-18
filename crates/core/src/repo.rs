use crate::{ProjectId, RepoId};

/// A git repository linked to a project. Unlike notes/attachments, the path
/// points at an arbitrary location on disk (the developer's existing
/// checkout) rather than somewhere inside the workspace data directory, so
/// there's no sandboxing to enforce here beyond "not empty".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub id: RepoId,
    pub project_id: ProjectId,
    pub path: String,
    pub name: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RepoError {
    #[error("repo path must not be empty")]
    EmptyPath,
    #[error("repo name must not be empty")]
    EmptyName,
}

impl Repo {
    pub fn new(
        id: RepoId,
        project_id: ProjectId,
        path: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, RepoError> {
        let path = path.into();
        let name = name.into();
        if path.trim().is_empty() {
            return Err(RepoError::EmptyPath);
        }
        if name.trim().is_empty() {
            return Err(RepoError::EmptyName);
        }
        Ok(Self {
            id,
            project_id,
            path,
            name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_path_or_name() {
        assert_eq!(
            Repo::new(RepoId(1), ProjectId(1), "", "name").unwrap_err(),
            RepoError::EmptyPath
        );
        assert_eq!(
            Repo::new(RepoId(1), ProjectId(1), "/some/path", "").unwrap_err(),
            RepoError::EmptyName
        );
    }

    #[test]
    fn new_accepts_valid_input() {
        let repo = Repo::new(RepoId(1), ProjectId(1), "/some/path", "myrepo").unwrap();
        assert_eq!(repo.path, "/some/path");
        assert_eq!(repo.name, "myrepo");
    }
}
