use crate::ProjectId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub description: Option<String>,
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProjectError {
    #[error("project name must not be empty")]
    EmptyName,
}

impl Project {
    /// Creates a new, unarchived project. `now` is a unix timestamp supplied
    /// by the caller so this stays pure and testable.
    pub fn new(id: ProjectId, name: impl Into<String>, now: i64) -> Result<Self, ProjectError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ProjectError::EmptyName);
        }
        Ok(Self {
            id,
            name,
            description: None,
            archived: false,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn archive(&mut self, now: i64) {
        self.archived = true;
        self.updated_at = now;
    }

    pub fn unarchive(&mut self, now: i64) {
        self.archived = false;
        self.updated_at = now;
    }

    pub fn rename(&mut self, name: impl Into<String>, now: i64) -> Result<(), ProjectError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ProjectError::EmptyName);
        }
        self.name = name;
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_name() {
        assert_eq!(
            Project::new(ProjectId(1), "  ", 0).unwrap_err(),
            ProjectError::EmptyName
        );
    }

    #[test]
    fn archive_and_unarchive_toggle_flag_and_timestamp() {
        let mut p = Project::new(ProjectId(1), "Devspace", 100).unwrap();
        p.archive(200);
        assert!(p.archived);
        assert_eq!(p.updated_at, 200);
        p.unarchive(300);
        assert!(!p.archived);
        assert_eq!(p.updated_at, 300);
    }

    #[test]
    fn rename_rejects_empty_name_and_keeps_old_value() {
        let mut p = Project::new(ProjectId(1), "Devspace", 0).unwrap();
        let err = p.rename("   ", 10).unwrap_err();
        assert_eq!(err, ProjectError::EmptyName);
        assert_eq!(p.name, "Devspace");
    }
}
