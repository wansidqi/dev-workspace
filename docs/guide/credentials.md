# Credentials vault

The vault keeps a project's passwords, tokens and keys, encrypted. Open it from **Credentials** in the sidebar (`Ctrl+Shift+L`). It needs a selected project, and each project has its own vault.

## How they are protected

- Every secret is encrypted with XChaCha20-Poly1305 and a fresh random nonce, and is tied to its project so a copied ciphertext does not open elsewhere.
- The key lives in your system's keychain (Windows Credential Manager, macOS Keychain or the Linux Secret Service), never in a file.
- Secrets are wiped from memory after use where possible, and are never written to logs or error messages.
- The vault is always drawn on an opaque surface, never see-through.

## Using it

- **Add.** Enter a label, choose the kind, optionally a username and a host, then the secret. The secret box shows one `*` per character and cannot be copied from.
- **Reveal.** Shows the value. It hides again by itself after 15 seconds, when you switch project and when you leave the view.
- **Copy.** Puts the value on the clipboard. The clipboard is cleared after 30 seconds if it still holds the secret.
- **Edit.** Change the label, username or host in place. To replace the secret, type the new value in the secret box and press **Replace**; an empty box keeps the old one.
- **Delete.** Click once, then confirm with a second click.

## Good to know

- Credentials are not part of a project export. A workspace backup includes them, but they are unreadable without the keychain key of the computer that made them. See [Settings and data](topic:settings).
- A secret can be up to 32 KiB and a label up to 100 characters.
- Injecting a credential into a terminal as an environment variable is not available yet.
