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

/// File extensions that would *run code* if handed to the OS "open" action
/// (executables, scripts, installers, shortcuts, archives that auto-run).
/// Attachments are untrusted input (CLAUDE.md), so these are revealed in the
/// file manager instead of launched.
const RISKY_EXTENSIONS: &[&str] = &[
    "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi",
    "msp", "scr", "pif", "lnk", "url", "reg", "hta", "cpl", "jar", "dll", "sh", "bash", "command",
    "app", "pkg", "dmg", "run", "bin", "appimage", "desktop", "scpt", "workflow",
];

/// `true` if opening `file_name` with the default handler could execute it.
/// Files with no extension count as risky too (on Unix they may be scripts).
pub fn is_launch_risky(file_name: &str) -> bool {
    let name = file_name.trim().trim_end_matches(['.', ' ']);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            RISKY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        }
        _ => true,
    }
}

/// Turns a user-visible file name into one safe to store under the
/// project's attachments directory: no path separators, no control or
/// reserved characters, never empty, bounded length. The original name is
/// kept separately for display.
pub fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        return "file".to_string();
    }
    // Keep the extension when truncating very long names.
    if cleaned.chars().count() > 120 {
        let ext = cleaned
            .rsplit_once('.')
            .map(|(_, e)| e.chars().take(10).collect::<String>());
        let stem: String = cleaned.chars().take(100).collect();
        return match ext {
            Some(e) if !e.is_empty() => format!("{stem}.{e}"),
            _ => stem,
        };
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executables_and_scripts_are_risky_to_launch_but_documents_are_not() {
        for risky in [
            "setup.exe",
            "RUN.BAT",
            "x.ps1",
            "a.JAR",
            "link.lnk",
            "noext",
            "x.exe.",
            "x.txt.exe",
        ] {
            assert!(is_launch_risky(risky), "{risky} should be risky");
        }
        for ok in [
            "spec.pdf",
            "diagram.PNG",
            "notes.md",
            "data.csv",
            "report.docx",
        ] {
            assert!(!is_launch_risky(ok), "{ok} should be safe to open");
        }
    }

    #[test]
    fn sanitize_strips_paths_and_reserved_characters() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(
            sanitize_file_name(r"C:\Users\me\spec v2.pdf"),
            "spec v2.pdf"
        );
        assert_eq!(sanitize_file_name("a<b>:c|d?.txt"), "a_b__c_d_.txt");
        assert_eq!(sanitize_file_name("..."), "file");
        assert_eq!(sanitize_file_name(""), "file");
        let long = format!("{}.pdf", "x".repeat(300));
        let out = sanitize_file_name(&long);
        assert!(out.ends_with(".pdf") && out.chars().count() <= 120);
    }

    #[test]
    fn sanitized_names_pass_the_relative_path_check_when_prefixed() {
        let stored = format!("7-{}", sanitize_file_name("../../x.pdf"));
        assert!(is_safe_relative_path(&stored));
    }

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
