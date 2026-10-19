//! Encryption for the credentials vault.
//!
//! Secrets are sealed with XChaCha20-Poly1305. Its 24-byte nonce is large
//! enough to draw at random for every secret without a realistic chance of
//! reuse. The 32-byte master key never touches the disk: it lives in the OS
//! keychain (Windows Credential Manager, macOS Keychain, Secret Service) and
//! is created on first use.
//!
//! Nothing here logs or formats a secret, and error messages never contain
//! one. Decrypted data comes back in [`Zeroizing`] so it is wiped on drop.

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

pub use zeroize::Zeroizing as Secret;

/// Keychain entry that holds the master key.
const KEYCHAIN_SERVICE: &str = "devspace";
const KEYCHAIN_USER: &str = "vault-master-key";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// The OS keychain could not be read or written.
    #[error("the OS keychain is unavailable: {0}")]
    KeyStore(String),
    /// Encryption failed (should not happen with a valid key).
    #[error("could not encrypt the secret")]
    Encrypt,
    /// The ciphertext did not authenticate: wrong key, tampered data or a
    /// row whose binding does not match.
    #[error("could not decrypt the secret (wrong key or damaged data)")]
    Decrypt,
}

/// Where the master key is kept.
pub trait KeyStore {
    /// Returns the master key, creating and saving a fresh random one the
    /// first time.
    fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError>;
}

/// The OS keychain.
pub struct KeyringStore;

impl KeyStore for KeyringStore {
    fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
            .map_err(|e| VaultError::KeyStore(e.to_string()))?;
        match entry.get_password() {
            Ok(stored) => {
                let stored = Zeroizing::new(stored);
                decode_key(&stored).ok_or_else(|| {
                    VaultError::KeyStore(
                        "the stored master key is damaged; it was not replaced so existing \
                         secrets are not lost"
                            .into(),
                    )
                })
            }
            Err(keyring::Error::NoEntry) => {
                let key = generate_key();
                let encoded = Zeroizing::new(encode_key(&key));
                entry
                    .set_password(&encoded)
                    .map_err(|e| VaultError::KeyStore(e.to_string()))?;
                Ok(key)
            }
            Err(e) => Err(VaultError::KeyStore(e.to_string())),
        }
    }
}

/// A key kept in memory only. For tests, where no keychain should be touched.
pub struct MemoryKeyStore(Zeroizing<[u8; KEY_LEN]>);

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self(generate_key())
    }
}

impl Default for MemoryKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for MemoryKeyStore {
    fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
        Ok(self.0.clone())
    }
}

fn generate_key() -> Zeroizing<[u8; KEY_LEN]> {
    let generated = XChaCha20Poly1305::generate_key(&mut OsRng);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&generated);
    key
}

fn encode_key(key: &[u8; KEY_LEN]) -> String {
    let mut out = String::with_capacity(KEY_LEN * 2);
    for byte in key {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn decode_key(hex: &str) -> Option<Zeroizing<[u8; KEY_LEN]>> {
    let hex = hex.trim();
    if hex.len() != KEY_LEN * 2 || !hex.is_ascii() {
        return None;
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        key[i] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(key)
}

/// A sealed secret: what the `credentials` row stores.
pub struct Sealed {
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// The unlocked vault: encrypts and decrypts secrets with the master key.
pub struct Vault {
    cipher: XChaCha20Poly1305,
}

impl Vault {
    /// Opens the vault with the key from `store`.
    pub fn open(store: &dyn KeyStore) -> Result<Self, VaultError> {
        let key = store.load_or_create()?;
        Ok(Self::from_key(&key))
    }

    /// Builds a vault from a raw key (tests, or a key you already hold).
    pub fn from_key(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new(key.into()),
        }
    }

    /// Seals `plaintext` under a fresh random nonce. `binding` is
    /// authenticated but not stored: decryption only works with the same
    /// binding, so a ciphertext copied into another row stops opening.
    pub fn seal(&self, binding: &[u8], plaintext: &[u8]) -> Result<Sealed, VaultError> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: binding,
                },
            )
            .map_err(|_| VaultError::Encrypt)?;
        Ok(Sealed {
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    /// Opens a secret sealed by [`Self::seal`] with the same `binding`.
    pub fn open_sealed(
        &self,
        binding: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        if nonce.len() != NONCE_LEN {
            return Err(VaultError::Decrypt);
        }
        self.cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: binding,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| VaultError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn vault() -> Vault {
        Vault::open(&MemoryKeyStore::new()).unwrap()
    }

    #[test]
    fn round_trip() {
        let vault = vault();
        let sealed = vault.seal(b"row-1", b"s3cret value").unwrap();
        assert_ne!(sealed.ciphertext, b"s3cret value");
        let opened = vault
            .open_sealed(b"row-1", &sealed.nonce, &sealed.ciphertext)
            .unwrap();
        assert_eq!(opened.as_slice(), b"s3cret value");
    }

    #[test]
    fn wrong_key_fails() {
        let sealed = vault().seal(b"row-1", b"s3cret").unwrap();
        let other = vault();
        assert!(other
            .open_sealed(b"row-1", &sealed.nonce, &sealed.ciphertext)
            .is_err());
    }

    #[test]
    fn a_different_binding_fails() {
        let vault = vault();
        let sealed = vault.seal(b"row-1", b"s3cret").unwrap();
        assert!(vault
            .open_sealed(b"row-2", &sealed.nonce, &sealed.ciphertext)
            .is_err());
    }

    #[test]
    fn tampering_and_bad_nonces_fail() {
        let vault = vault();
        let mut sealed = vault.seal(b"b", b"s3cret").unwrap();
        sealed.ciphertext[0] ^= 1;
        assert!(vault
            .open_sealed(b"b", &sealed.nonce, &sealed.ciphertext)
            .is_err());
        assert!(vault
            .open_sealed(b"b", &[0u8; 3], &sealed.ciphertext)
            .is_err());
    }

    #[test]
    fn a_nonce_is_never_reused() {
        let vault = vault();
        let mut seen = HashSet::new();
        for _ in 0..2000 {
            let sealed = vault.seal(b"b", b"same plaintext").unwrap();
            assert_eq!(sealed.nonce.len(), NONCE_LEN);
            assert!(seen.insert(sealed.nonce), "nonce repeated");
        }
    }

    #[test]
    fn key_hex_round_trips_and_rejects_garbage() {
        let key = generate_key();
        let decoded = decode_key(&encode_key(&key)).unwrap();
        assert_eq!(*decoded, *key);
        assert!(decode_key("").is_none());
        assert!(decode_key(&"zz".repeat(KEY_LEN)).is_none());
        assert!(decode_key(&"ab".repeat(KEY_LEN - 1)).is_none());
        assert!(decode_key(&"é".repeat(KEY_LEN)).is_none());
    }

    #[test]
    fn memory_store_keeps_one_key() {
        let store = MemoryKeyStore::new();
        assert_eq!(
            *store.load_or_create().unwrap(),
            *store.load_or_create().unwrap()
        );
        assert_ne!(*generate_key(), *generate_key());
    }
}

#[cfg(test)]
mod keychain_tests {
    use super::*;

    /// Touches the real OS keychain, with a throwaway entry (never the vault's
    /// own), so it only runs on request: `cargo test -p devspace-storage --
    /// --ignored keychain`.
    #[test]
    #[ignore = "uses the OS keychain"]
    fn keychain_round_trips_a_key_in_a_throwaway_entry() {
        let entry = keyring::Entry::new("devspace-test", "keychain-round-trip").unwrap();
        let key = generate_key();
        entry.set_password(&encode_key(&key)).unwrap();
        let stored = Zeroizing::new(entry.get_password().unwrap());
        assert_eq!(*decode_key(&stored).unwrap(), *key);
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    }
}
