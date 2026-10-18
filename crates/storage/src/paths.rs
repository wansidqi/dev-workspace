use std::path::{Path, PathBuf};

use devspace_core::ProjectId;

/// Resolves the on-disk layout described in CLAUDE.md:
/// `<data_dir>/devspace/{app.db, projects/<id>/{notes,attachments}, backups}`.
/// Never hardcode these paths elsewhere — go through this type.
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    root: PathBuf,
}

impl WorkspacePaths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Uses the OS data directory via the `dirs` crate.
    pub fn from_os_data_dir() -> Option<Self> {
        dirs::data_dir().map(|d| Self::new(d.join("devspace")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn database_file(&self) -> PathBuf {
        self.root.join("app.db")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    pub fn project_dir(&self, id: ProjectId) -> PathBuf {
        self.root.join("projects").join(id.0.to_string())
    }

    pub fn notes_dir(&self, id: ProjectId) -> PathBuf {
        self.project_dir(id).join("notes")
    }

    pub fn attachments_dir(&self, id: ProjectId) -> PathBuf {
        self.project_dir(id).join("attachments")
    }

    /// Creates the root, backups and (if given) project subdirectories.
    pub fn ensure_dirs(&self, project: Option<ProjectId>) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(self.backups_dir())?;
        if let Some(id) = project {
            std::fs::create_dir_all(self.notes_dir(id))?;
            std::fs::create_dir_all(self.attachments_dir(id))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_paths_nest_under_root() {
        let paths = WorkspacePaths::new(PathBuf::from("/data/devspace"));
        assert_eq!(
            paths.notes_dir(ProjectId(7)),
            PathBuf::from("/data/devspace/projects/7/notes")
        );
    }
}
