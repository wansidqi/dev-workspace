use crate::{NoteId, ProjectId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    Note,
    Script,
    Sql,
}

impl NoteKind {
    /// Short name shown on chips and badges.
    pub fn label(self) -> &'static str {
        match self {
            NoteKind::Note => "Note",
            NoteKind::Script => "Script",
            NoteKind::Sql => "SQL",
        }
    }

    /// The kind a click on the kind chip moves to, wrapping around.
    pub fn cycle(self) -> Self {
        match self {
            NoteKind::Note => NoteKind::Script,
            NoteKind::Script => NoteKind::Sql,
            NoteKind::Sql => NoteKind::Note,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    pub project_id: ProjectId,
    pub task_id: Option<TaskId>,
    pub kind: NoteKind,
    pub title: String,
    /// Path to the markdown/script file on disk, relative to the project's
    /// `notes/` directory. Content itself lives in plain files, not the db.
    pub file_path: String,
    pub updated_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NoteError {
    #[error("note title must not be empty")]
    EmptyTitle,
    #[error("note file path must be a relative path with no '..' segments")]
    InvalidPath,
}

impl Note {
    pub fn new(
        id: NoteId,
        project_id: ProjectId,
        kind: NoteKind,
        title: impl Into<String>,
        file_path: impl Into<String>,
        now: i64,
    ) -> Result<Self, NoteError> {
        let title = title.into();
        let file_path = file_path.into();
        if title.trim().is_empty() {
            return Err(NoteError::EmptyTitle);
        }
        if !is_safe_relative_path(&file_path) {
            return Err(NoteError::InvalidPath);
        }
        Ok(Self {
            id,
            project_id,
            task_id: None,
            kind,
            title,
            file_path,
            updated_at: now,
        })
    }
}

/// Rejects absolute paths and any `..` segment so notes/attachments can
/// never escape the project directory (see CLAUDE.md security rules).
pub fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let p = std::path::Path::new(path);
    // `has_root` also catches rooted-but-driveless paths like `/etc/passwd`,
    // which `is_absolute` treats as relative on Windows.
    if p.is_absolute() || p.has_root() {
        return false;
    }
    !p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_cycle_visits_every_kind_and_wraps() {
        let mut kind = NoteKind::Note;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(kind.label());
            kind = kind.cycle();
        }
        assert_eq!(seen, ["Note", "Script", "SQL"]);
        assert_eq!(kind, NoteKind::Note);
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        assert!(!is_safe_relative_path("../../etc/passwd"));
        assert!(!is_safe_relative_path("notes/../../secret.md"));
    }

    #[test]
    fn rejects_absolute_paths() {
        assert!(!is_safe_relative_path("/etc/passwd"));
    }

    #[test]
    fn accepts_plain_relative_paths() {
        assert!(is_safe_relative_path("notes/todo.md"));
    }

    #[test]
    fn new_rejects_unsafe_path() {
        let err = Note::new(
            NoteId(1),
            ProjectId(1),
            NoteKind::Note,
            "Todo",
            "../escape.md",
            0,
        )
        .unwrap_err();
        assert_eq!(err, NoteError::InvalidPath);
    }
}
