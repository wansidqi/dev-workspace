use crate::{note::is_safe_relative_path, AttachmentId, ProjectId, TaskId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: AttachmentId,
    pub project_id: ProjectId,
    pub task_id: Option<TaskId>,
    pub name: String,
    pub file_path: String,
    /// `true` if the file was copied into the project's attachments
    /// directory; `false` if it is only a link to a path elsewhere.
    pub is_linked: bool,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AttachmentError {
    #[error("attachment name must not be empty")]
    EmptyName,
    #[error("copied attachments must use a path inside the project directory")]
    InvalidPath,
}

impl Attachment {
    pub fn new(
        id: AttachmentId,
        project_id: ProjectId,
        name: impl Into<String>,
        file_path: impl Into<String>,
        is_linked: bool,
        now: i64,
    ) -> Result<Self, AttachmentError> {
        let name = name.into();
        let file_path = file_path.into();
        if name.trim().is_empty() {
            return Err(AttachmentError::EmptyName);
        }
        // Only copied-in attachments are constrained to the project
        // directory; linked attachments intentionally point elsewhere.
        if !is_linked && !is_safe_relative_path(&file_path) {
            return Err(AttachmentError::InvalidPath);
        }
        Ok(Self {
            id,
            project_id,
            task_id: None,
            name,
            file_path,
            is_linked,
            created_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_attachment_rejects_traversal() {
        let err = Attachment::new(
            AttachmentId(1),
            ProjectId(1),
            "evil",
            "../../secret.pdf",
            false,
            0,
        )
        .unwrap_err();
        assert_eq!(err, AttachmentError::InvalidPath);
    }

    #[test]
    fn linked_attachment_allows_external_path() {
        let a = Attachment::new(
            AttachmentId(1),
            ProjectId(1),
            "spec",
            "C:/Users/me/spec.pdf",
            true,
            0,
        )
        .unwrap();
        assert!(a.is_linked);
    }
}
