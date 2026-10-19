//! Credentials-vault helpers of `AppState`, in their own file (a child module
//! of `state`, so it can use the private connection and vault).
//!
//! Only metadata lives in `AppState::credentials`. A secret is decrypted
//! just in time by [`AppState::reveal_credential`] and comes back in a
//! wrapper that wipes it when dropped. Nothing here logs a value.

use super::{now, AppState};
use devspace_core::{CredentialId, CredentialKind, CredentialMeta};
use devspace_storage::{CredentialRepo, KeyringStore, Secret, Vault};

impl AppState {
    /// The unlocked vault, opened on first use (this is what reads or creates
    /// the master key in the OS keychain).
    fn open_vault(&mut self) -> Result<&Vault, String> {
        if self.vault.is_none() {
            let vault = Vault::open(&KeyringStore).map_err(|e| e.to_string())?;
            self.vault = Some(vault);
        }
        self.vault
            .as_ref()
            .ok_or_else(|| "The vault is unavailable".to_string())
    }

    /// Reloads the selected project's credential metadata. Secrets are not read.
    pub(super) fn reload_credentials(&mut self) {
        let Some(project) = self.selected_project else {
            self.credentials.clear();
            return;
        };
        self.credentials =
            CredentialRepo::list_for_project(&self.conn, project).unwrap_or_default();
        if let Some(max) = self.credentials.iter().map(|c| c.id.0).max() {
            self.next_credential_id = self.next_credential_id.max(max + 1);
        }
    }

    /// Stores a new encrypted credential on the selected project.
    pub fn add_credential(
        &mut self,
        label: &str,
        kind: CredentialKind,
        value: &str,
    ) -> Result<CredentialId, String> {
        let project = self
            .selected_project
            .ok_or_else(|| "Select a project first".to_string())?;
        let id = CredentialId(Self::next_id(
            &mut self.next_credential_id,
            self.credentials.iter().map(|c| c.id.0).max().unwrap_or(0),
        ));
        let meta =
            CredentialMeta::new(id, project, label, kind, now()).map_err(|e| e.to_string())?;
        self.open_vault()?;
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| "The vault is unavailable".to_string())?;
        CredentialRepo::insert(&self.conn, vault, &meta, value).map_err(|e| e.to_string())?;
        self.credentials.push(meta);
        Ok(id)
    }

    /// Decrypts one credential of the selected project. The caller owns the
    /// returned secret and should drop it as soon as it is no longer shown.
    pub fn reveal_credential(&mut self, id: CredentialId) -> Result<Secret<String>, String> {
        if !self.credentials.iter().any(|c| c.id == id) {
            return Err("That credential is not in this project".into());
        }
        self.open_vault()?;
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| "The vault is unavailable".to_string())?;
        CredentialRepo::reveal(&self.conn, vault, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "That credential no longer exists".to_string())
    }

    /// Replaces the secret of a credential of the selected project.
    pub fn replace_credential_value(
        &mut self,
        id: CredentialId,
        value: &str,
    ) -> Result<(), String> {
        if !self.credentials.iter().any(|c| c.id == id) {
            return Err("That credential is not in this project".into());
        }
        self.open_vault()?;
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| "The vault is unavailable".to_string())?;
        match CredentialRepo::replace_value(&self.conn, vault, id, value) {
            Ok(true) => Ok(()),
            Ok(false) => Err("That credential no longer exists".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Deletes a credential of the selected project.
    pub fn delete_credential(&mut self, id: CredentialId) {
        if !self.credentials.iter().any(|c| c.id == id) {
            return;
        }
        if CredentialRepo::delete(&self.conn, id).is_ok() {
            self.credentials.retain(|c| c.id != id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devspace_storage::{open_in_memory, run_migrations, MemoryKeyStore, WorkspacePaths};

    /// A state with a throwaway in-memory key, so no test touches the keychain.
    fn state() -> AppState {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        let root = std::env::temp_dir().join("devspace-credentials-test-unused");
        let mut state = AppState::new(conn, WorkspacePaths::new(root));
        state.vault = Some(Vault::open(&MemoryKeyStore::new()).unwrap());
        state.create_project("P");
        state
    }

    #[test]
    fn add_reveal_and_delete_round_trip() {
        let mut state = state();
        let id = state
            .add_credential("  prod db  ", CredentialKind::Password, "hunter2")
            .unwrap();
        assert_eq!(state.credentials[0].label, "prod db");
        assert_eq!(state.reveal_credential(id).unwrap().as_str(), "hunter2");

        // Survives a reselect (metadata reloads from the database).
        let project = state.selected_project.unwrap();
        state.select_project(project);
        assert_eq!(state.credentials.len(), 1);
        assert_eq!(state.reveal_credential(id).unwrap().as_str(), "hunter2");

        state.replace_credential_value(id, "rotated").unwrap();
        assert_eq!(state.reveal_credential(id).unwrap().as_str(), "rotated");

        state.delete_credential(id);
        assert!(state.credentials.is_empty());
        assert!(state.reveal_credential(id).is_err());
    }

    #[test]
    fn bad_input_is_refused_without_storing_anything() {
        let mut state = state();
        assert!(state
            .add_credential("", CredentialKind::Token, "x")
            .is_err());
        assert!(state
            .add_credential("a", CredentialKind::Token, "")
            .is_err());
        assert!(state.credentials.is_empty());
        // The error text never carries the value.
        let err = state
            .add_credential("", CredentialKind::Token, "super-secret-value")
            .unwrap_err();
        assert!(!err.contains("super-secret-value"));
    }

    #[test]
    fn credentials_are_per_project() {
        let mut state = state();
        let a = state.selected_project.unwrap();
        let id = state
            .add_credential("a-key", CredentialKind::ApiKey, "aaa")
            .unwrap();
        state.create_project("Q");
        assert!(state.credentials.is_empty());
        // Another project's credential cannot be revealed or deleted from here.
        assert!(state.reveal_credential(id).is_err());
        state.delete_credential(id);
        state.select_project(a);
        assert_eq!(state.reveal_credential(id).unwrap().as_str(), "aaa");
    }

    #[test]
    fn ids_never_repeat_after_a_delete() {
        let mut state = state();
        let first = state
            .add_credential("one", CredentialKind::Other, "1")
            .unwrap();
        let second = state
            .add_credential("two", CredentialKind::Other, "2")
            .unwrap();
        state.delete_credential(second);
        let third = state
            .add_credential("three", CredentialKind::Other, "3")
            .unwrap();
        assert!(first.0 < second.0 && second.0 < third.0);
    }
}
