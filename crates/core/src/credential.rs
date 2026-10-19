use crate::{CredentialId, ProjectId};

/// Longest label, in characters.
pub const MAX_CREDENTIAL_LABEL_CHARS: usize = 100;

/// Largest secret the vault accepts, in bytes. Room for a private key file,
/// small enough that a paste mistake is refused instead of stored.
pub const MAX_CREDENTIAL_VALUE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Password,
    ApiKey,
    Token,
    SshKey,
    Other,
}

impl CredentialKind {
    /// Every kind, in the order the kind chip cycles through them.
    pub const ALL: [CredentialKind; 5] = [
        CredentialKind::Password,
        CredentialKind::ApiKey,
        CredentialKind::Token,
        CredentialKind::SshKey,
        CredentialKind::Other,
    ];

    /// Name shown on chips and badges.
    pub fn label(self) -> &'static str {
        match self {
            CredentialKind::Password => "Password",
            CredentialKind::ApiKey => "API key",
            CredentialKind::Token => "Token",
            CredentialKind::SshKey => "SSH key",
            CredentialKind::Other => "Other",
        }
    }

    /// The kind a click on the kind chip moves to, wrapping around.
    pub fn cycle(self) -> Self {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
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
    #[error("credential label is longer than {MAX_CREDENTIAL_LABEL_CHARS} characters")]
    LabelTooLong,
    #[error("credential label must not contain control characters")]
    LabelControlChar,
    #[error("the secret value must not be empty")]
    EmptyValue,
    #[error("the secret value is larger than {} KiB", MAX_CREDENTIAL_VALUE_BYTES / 1024)]
    ValueTooLarge,
}

/// Checks a secret before it is stored. The message never includes the value.
pub fn validate_credential_value(value: &str) -> Result<(), CredentialError> {
    if value.is_empty() {
        return Err(CredentialError::EmptyValue);
    }
    if value.len() > MAX_CREDENTIAL_VALUE_BYTES {
        return Err(CredentialError::ValueTooLarge);
    }
    Ok(())
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
        let label = label.trim().to_string();
        if label.is_empty() {
            return Err(CredentialError::EmptyLabel);
        }
        if label.chars().any(char::is_control) {
            return Err(CredentialError::LabelControlChar);
        }
        if label.chars().count() > MAX_CREDENTIAL_LABEL_CHARS {
            return Err(CredentialError::LabelTooLong);
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

    fn meta(label: &str) -> Result<CredentialMeta, CredentialError> {
        CredentialMeta::new(
            CredentialId(1),
            ProjectId(1),
            label,
            CredentialKind::Token,
            0,
        )
    }

    #[test]
    fn new_rejects_empty_label() {
        assert_eq!(meta("").unwrap_err(), CredentialError::EmptyLabel);
        assert_eq!(meta("   ").unwrap_err(), CredentialError::EmptyLabel);
    }

    #[test]
    fn label_is_trimmed_and_checked() {
        assert_eq!(meta("  prod db  ").unwrap().label, "prod db");
        assert_eq!(meta("a\nb").unwrap_err(), CredentialError::LabelControlChar);
        assert_eq!(
            meta(&"x".repeat(MAX_CREDENTIAL_LABEL_CHARS + 1)).unwrap_err(),
            CredentialError::LabelTooLong
        );
        assert!(meta(&"x".repeat(MAX_CREDENTIAL_LABEL_CHARS)).is_ok());
    }

    #[test]
    fn value_limits() {
        assert_eq!(
            validate_credential_value("").unwrap_err(),
            CredentialError::EmptyValue
        );
        assert!(validate_credential_value("hunter2").is_ok());
        assert_eq!(
            validate_credential_value(&"x".repeat(MAX_CREDENTIAL_VALUE_BYTES + 1)).unwrap_err(),
            CredentialError::ValueTooLarge
        );
    }

    #[test]
    fn kind_cycle_visits_every_kind_and_wraps() {
        let mut kind = CredentialKind::Password;
        let mut seen = Vec::new();
        for _ in 0..CredentialKind::ALL.len() {
            seen.push(kind);
            kind = kind.cycle();
        }
        assert_eq!(seen, CredentialKind::ALL);
        assert_eq!(kind, CredentialKind::Password);
    }
}
