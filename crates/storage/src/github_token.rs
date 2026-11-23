//! The GitHub access token, kept in the OS keychain next to the vault's
//! master key and never written to disk, a log or an error message.

use zeroize::Zeroizing;

const SERVICE: &str = "devspace";
const USER: &str = "github-token";

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, USER).map_err(|e| e.to_string())
}

/// Saves (replacing) the token.
pub fn save_github_token(token: &str) -> Result<(), String> {
    entry()?.set_password(token).map_err(|e| e.to_string())
}

/// The saved token, if any. A keychain that can't be read counts as none.
pub fn load_github_token() -> Option<Zeroizing<String>> {
    entry().ok()?.get_password().ok().map(Zeroizing::new)
}

/// Forgets the token (sign out). Succeeds when there was none.
pub fn clear_github_token() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
