use devspace_core::{
    validate_credential_value, CredentialId, CredentialKind, CredentialMeta, ProjectId,
};
use rusqlite::{params, Connection, OptionalExtension};
use zeroize::Zeroizing;

use crate::vault::Vault;
use crate::StorageError;

/// Encrypted per-project secrets. Metadata queries never select the
/// ciphertext; only [`CredentialRepo::reveal`] reads it.
pub struct CredentialRepo;

/// What a row's ciphertext is bound to, so it cannot be moved to another
/// credential or project and still open.
fn binding(project: ProjectId, id: CredentialId) -> Vec<u8> {
    let mut out = b"devspace-credential".to_vec();
    out.extend_from_slice(&project.0.to_le_bytes());
    out.extend_from_slice(&id.0.to_le_bytes());
    out
}

fn kind_to_str(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::Password => "password",
        CredentialKind::ApiKey => "api_key",
        CredentialKind::Token => "token",
        CredentialKind::SshKey => "ssh_key",
        CredentialKind::Other => "other",
    }
}

fn str_to_kind(s: &str) -> CredentialKind {
    match s {
        "password" => CredentialKind::Password,
        "api_key" => CredentialKind::ApiKey,
        "token" => CredentialKind::Token,
        "ssh_key" => CredentialKind::SshKey,
        _ => CredentialKind::Other,
    }
}

fn domain(e: impl std::error::Error + Send + Sync + 'static) -> StorageError {
    StorageError::Domain(anyhow::Error::new(e))
}

impl CredentialRepo {
    /// Encrypts `value` and stores it with `meta`. The value is not kept.
    pub fn insert(
        conn: &Connection,
        vault: &Vault,
        meta: &CredentialMeta,
        value: &str,
    ) -> Result<(), StorageError> {
        validate_credential_value(value).map_err(domain)?;
        let sealed = vault
            .seal(&binding(meta.project_id, meta.id), value.as_bytes())
            .map_err(domain)?;
        conn.execute(
            "INSERT INTO credentials (id, project_id, label, kind, nonce, ciphertext, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                meta.id.0,
                meta.project_id.0,
                meta.label,
                kind_to_str(meta.kind),
                sealed.nonce,
                sealed.ciphertext,
                meta.created_at,
            ],
        )?;
        Ok(())
    }

    /// Credentials of a project, oldest first. No secret data is read.
    pub fn list_for_project(
        conn: &Connection,
        project: ProjectId,
    ) -> Result<Vec<CredentialMeta>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, label, kind, created_at
             FROM credentials WHERE project_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(params![project.0], |row| {
            Ok(CredentialMeta {
                id: CredentialId(row.get(0)?),
                project_id: ProjectId(row.get(1)?),
                label: row.get(2)?,
                kind: str_to_kind(&row.get::<_, String>(3)?),
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<_, _>>().map_err(StorageError::from)
    }

    /// Decrypts one secret. `None` when the credential does not exist. The
    /// returned text is wiped when dropped.
    pub fn reveal(
        conn: &Connection,
        vault: &Vault,
        id: CredentialId,
    ) -> Result<Option<Zeroizing<String>>, StorageError> {
        let row = conn
            .query_row(
                "SELECT project_id, nonce, ciphertext FROM credentials WHERE id = ?1",
                params![id.0],
                |row| {
                    Ok((
                        ProjectId(row.get(0)?),
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((project, nonce, ciphertext)) = row else {
            return Ok(None);
        };
        let plain = vault
            .open_sealed(&binding(project, id), &nonce, &ciphertext)
            .map_err(domain)?;
        // `from_utf8` hands the bytes back on failure, still wrapped so they
        // are wiped.
        let bytes = Zeroizing::new(plain.to_vec());
        let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
            StorageError::Domain(anyhow::anyhow!("the stored secret is not valid text"))
        })?;
        Ok(Some(Zeroizing::new(text)))
    }

    /// Replaces a secret with a new value (a fresh nonce each time).
    pub fn replace_value(
        conn: &Connection,
        vault: &Vault,
        id: CredentialId,
        value: &str,
    ) -> Result<bool, StorageError> {
        validate_credential_value(value).map_err(domain)?;
        let project: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM credentials WHERE id = ?1",
                params![id.0],
                |row| row.get(0),
            )
            .optional()?;
        let Some(project) = project else {
            return Ok(false);
        };
        let sealed = vault
            .seal(&binding(ProjectId(project), id), value.as_bytes())
            .map_err(domain)?;
        let changed = conn.execute(
            "UPDATE credentials SET nonce = ?2, ciphertext = ?3 WHERE id = ?1",
            params![id.0, sealed.nonce, sealed.ciphertext],
        )?;
        Ok(changed > 0)
    }

    pub fn delete(conn: &Connection, id: CredentialId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM credentials WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    /// Largest credential id in the database, for the id counter.
    pub fn max_id(conn: &Connection) -> Result<i64, StorageError> {
        let max: Option<i64> =
            conn.query_row("SELECT MAX(id) FROM credentials", [], |row| row.get(0))?;
        Ok(max.unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::MemoryKeyStore;
    use crate::{open_in_memory, run_migrations};
    use devspace_core::Project;

    fn setup() -> (Connection, Vault) {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for id in [1, 2] {
            let project = Project::new(devspace_core::ProjectId(id), format!("P{id}"), 0).unwrap();
            crate::ProjectRepo::insert(&conn, &project).unwrap();
        }
        (conn, Vault::open(&MemoryKeyStore::new()).unwrap())
    }

    fn meta(id: i64, project: i64, label: &str) -> CredentialMeta {
        CredentialMeta::new(
            CredentialId(id),
            ProjectId(project),
            label,
            CredentialKind::ApiKey,
            id,
        )
        .unwrap()
    }

    #[test]
    fn stores_lists_reveals_and_deletes() {
        let (conn, vault) = setup();
        CredentialRepo::insert(&conn, &vault, &meta(1, 1, "prod db"), "hunter2").unwrap();
        CredentialRepo::insert(&conn, &vault, &meta(2, 1, "stripe"), "sk_live_x").unwrap();
        CredentialRepo::insert(&conn, &vault, &meta(3, 2, "other project"), "zzz").unwrap();

        let listed = CredentialRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(
            listed.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            ["prod db", "stripe"]
        );
        assert_eq!(listed[0].kind, CredentialKind::ApiKey);

        let revealed = CredentialRepo::reveal(&conn, &vault, CredentialId(2))
            .unwrap()
            .unwrap();
        assert_eq!(revealed.as_str(), "sk_live_x");
        assert!(CredentialRepo::reveal(&conn, &vault, CredentialId(99))
            .unwrap()
            .is_none());

        CredentialRepo::delete(&conn, CredentialId(1)).unwrap();
        assert_eq!(
            CredentialRepo::list_for_project(&conn, ProjectId(1))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(CredentialRepo::max_id(&conn).unwrap(), 3);
    }

    #[test]
    fn the_database_never_holds_the_plaintext() {
        let (conn, vault) = setup();
        let secret = "correct horse battery staple";
        CredentialRepo::insert(&conn, &vault, &meta(1, 1, "x"), secret).unwrap();
        let (nonce, ciphertext): (Vec<u8>, Vec<u8>) = conn
            .query_row(
                "SELECT nonce, ciphertext FROM credentials WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let contains = |haystack: &[u8]| {
            haystack
                .windows(secret.len())
                .any(|w| w == secret.as_bytes())
        };
        assert!(!contains(&ciphertext));
        assert!(!contains(&nonce));
    }

    #[test]
    fn a_wrong_key_cannot_read_and_a_moved_ciphertext_does_not_open() {
        let (conn, vault) = setup();
        CredentialRepo::insert(&conn, &vault, &meta(1, 1, "a"), "alpha").unwrap();
        CredentialRepo::insert(&conn, &vault, &meta(2, 1, "b"), "beta").unwrap();

        let other = Vault::open(&MemoryKeyStore::new()).unwrap();
        assert!(CredentialRepo::reveal(&conn, &other, CredentialId(1)).is_err());

        // Copy row 1's sealed data over row 2: the binding no longer matches.
        conn.execute(
            "UPDATE credentials SET nonce = (SELECT nonce FROM credentials WHERE id = 1),
                                    ciphertext = (SELECT ciphertext FROM credentials WHERE id = 1)
             WHERE id = 2",
            [],
        )
        .unwrap();
        assert!(CredentialRepo::reveal(&conn, &vault, CredentialId(2)).is_err());
        assert_eq!(
            CredentialRepo::reveal(&conn, &vault, CredentialId(1))
                .unwrap()
                .unwrap()
                .as_str(),
            "alpha"
        );
    }

    #[test]
    fn replacing_a_value_uses_a_new_nonce_and_keeps_the_label() {
        let (conn, vault) = setup();
        CredentialRepo::insert(&conn, &vault, &meta(1, 1, "a"), "old").unwrap();
        let nonce = |conn: &Connection| -> Vec<u8> {
            conn.query_row("SELECT nonce FROM credentials WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let before = nonce(&conn);
        assert!(CredentialRepo::replace_value(&conn, &vault, CredentialId(1), "new").unwrap());
        assert_ne!(before, nonce(&conn));
        assert_eq!(
            CredentialRepo::reveal(&conn, &vault, CredentialId(1))
                .unwrap()
                .unwrap()
                .as_str(),
            "new"
        );
        assert!(!CredentialRepo::replace_value(&conn, &vault, CredentialId(9), "x").unwrap());
    }

    #[test]
    fn empty_and_oversized_values_are_refused_and_nothing_is_stored() {
        let (conn, vault) = setup();
        assert!(CredentialRepo::insert(&conn, &vault, &meta(1, 1, "a"), "").is_err());
        let huge = "x".repeat(devspace_core::MAX_CREDENTIAL_VALUE_BYTES + 1);
        assert!(CredentialRepo::insert(&conn, &vault, &meta(1, 1, "a"), &huge).is_err());
        assert!(CredentialRepo::list_for_project(&conn, ProjectId(1))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn deleting_a_project_deletes_its_credentials() {
        let (conn, vault) = setup();
        CredentialRepo::insert(&conn, &vault, &meta(1, 2, "a"), "alpha").unwrap();
        conn.execute("DELETE FROM projects WHERE id = 2", [])
            .unwrap();
        assert_eq!(CredentialRepo::max_id(&conn).unwrap(), 0);
    }
}
