use crate::{CredentialId, ProjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Password,
    ApiKey,
    Token,
    SshKey,
    Other,
}

/// Metadata about a stored credential. The plaintext value never lives in
/// this struct or anywhere in `core` — decryption happens in `storage`,
/// just-in-time, and the result must be zeroized after use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMeta {
    pub id: CredentialId,
    pub project_id: ProjectId,
    pub label: String,
    pub kind: CredentialKind,
    pub created_at: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("credential label must not be empty")]
    EmptyLabel,
}

impl CredentialMeta {
    pub fn new(
        id: CredentialId,
        project_id: ProjectId,
        label: impl Into<String>,
        kind: CredentialKind,
        now: i64,
    ) -> Result<Self, CredentialError> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(CredentialError::EmptyLabel);
        }
        Ok(Self {
            id,
            project_id,
            label,
            kind,
            created_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_label() {
        assert_eq!(
            CredentialMeta::new(CredentialId(1), ProjectId(1), "", CredentialKind::Token, 0)
                .unwrap_err(),
            CredentialError::EmptyLabel
        );
    }
}
