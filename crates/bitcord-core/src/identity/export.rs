//! Portable identity export/import for cross-device identity sharing.
//!
//! # Format (`.bcid` file)
//!
//! ```text
//! [0..4]   magic: b"BCID"
//! [4]      version: 1
//! [5]      display_name length (0–64)
//! [6..6+N] display_name (UTF-8, plaintext)
//! [6+N..]  104-byte KeyStore blob (32-byte salt + 24-byte nonce + 48-byte ciphertext)
//! ```
//!
//! The 104-byte KeyStore blob is XChaCha20-Poly1305 AEAD-authenticated with a
//! key derived from the export passphrase via Argon2id.  A wrong export
//! passphrase produces a decryption error; the format is self-contained and
//! does **not** rely on the local device's keystore file.

use anyhow::{bail, Context};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{XChaCha20Poly1305, XNonce, aead::{Aead, KeyInit}};
use rand::{RngCore, rngs::OsRng};
use zeroize::Zeroize;

use super::NodeIdentity;

/// Magic bytes that identify a BitCord identity export file.
const MAGIC: &[u8; 4] = b"BCID";
const VERSION: u8 = 1;

/// Argon2id parameters — match `KeyStore` for consistent security.
const ARGON2_M_COST: u32 = 65_536;
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;

/// A portable, passphrase-encrypted bundle that carries a `NodeIdentity` and
/// an optional display name.  Use [`IdentityExport::create`] to produce a
/// bundle for export and [`IdentityExport::load`] to recover the identity on
/// another device.
pub struct IdentityExport;

impl IdentityExport {
    /// Serialize `identity` (plus `display_name`) into a portable byte bundle
    /// encrypted under `export_passphrase`.
    ///
    /// The returned `Vec<u8>` is suitable for writing to a `.bcid` file.
    pub fn create(
        identity: &NodeIdentity,
        display_name: Option<&str>,
        export_passphrase: &str,
    ) -> anyhow::Result<Vec<u8>> {
        // The on-disk format stores the name length in a single byte, so the
        // name must fit in 64 bytes. Truncate on a UTF-8 char boundary rather
        // than erroring, so multibyte names (e.g. emoji) always export cleanly.
        let name = truncate_to_byte_len(display_name.unwrap_or(""), 64);

        let keystore_bytes = encrypt_signing_key(identity, export_passphrase)?;

        let name_bytes = name.as_bytes();
        let mut out = Vec::with_capacity(6 + name_bytes.len() + keystore_bytes.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(name_bytes.len() as u8);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&keystore_bytes);

        Ok(out)
    }

    /// Recover a `NodeIdentity` (and optional display name) from a byte bundle
    /// produced by [`IdentityExport::create`].
    ///
    /// Returns `(identity, display_name)`.  `display_name` is `None` if the
    /// bundle was created without one.
    pub fn load(
        bytes: &[u8],
        export_passphrase: &str,
    ) -> anyhow::Result<(NodeIdentity, Option<String>)> {
        if bytes.len() < 6 {
            bail!("not a valid BitCord identity file (too short)");
        }
        if &bytes[0..4] != MAGIC {
            bail!("not a valid BitCord identity file (wrong magic bytes)");
        }
        if bytes[4] != VERSION {
            bail!("unsupported BitCord identity file version {}", bytes[4]);
        }

        let name_len = bytes[5] as usize;
        let header_end = 6 + name_len;
        if bytes.len() < header_end + 104 {
            bail!("BitCord identity file is truncated");
        }

        let display_name = if name_len == 0 {
            None
        } else {
            Some(
                std::str::from_utf8(&bytes[6..header_end])
                    .context("display_name in export file is not valid UTF-8")?
                    .to_owned(),
            )
        };

        let keystore_bytes = &bytes[header_end..header_end + 104];
        let identity = decrypt_signing_key(keystore_bytes, export_passphrase)
            .context("failed to decrypt identity export (wrong passphrase or corrupt file)")?;

        Ok((identity, display_name))
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Produce a 104-byte KeyStore-compatible blob (salt + nonce + ciphertext).
fn encrypt_signing_key(identity: &NodeIdentity, passphrase: &str) -> anyhow::Result<Vec<u8>> {
    let mut salt = [0u8; 32];
    OsRng.fill_bytes(&mut salt);

    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);

    let mut okm = [0u8; 32];
    derive_key(passphrase, &salt, &mut okm);

    let cipher = XChaCha20Poly1305::new_from_slice(&okm)
        .context("failed to create cipher")?;
    okm.zeroize();

    let nonce = XNonce::from_slice(&nonce_bytes);
    let signing_bytes = identity.signing_key_bytes();
    let ciphertext = cipher
        .encrypt(nonce, signing_bytes.as_ref())
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut out = Vec::with_capacity(32 + 24 + ciphertext.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a 104-byte KeyStore-compatible blob back to a `NodeIdentity`.
fn decrypt_signing_key(blob: &[u8], passphrase: &str) -> anyhow::Result<NodeIdentity> {
    if blob.len() < 56 {
        bail!("keystore blob is too short");
    }

    let salt = &blob[0..32];
    let nonce_bytes = &blob[32..56];
    let ciphertext = &blob[56..];

    let mut okm = [0u8; 32];
    derive_key(passphrase, salt, &mut okm);

    let cipher = XChaCha20Poly1305::new_from_slice(&okm)
        .context("failed to create cipher")?;
    okm.zeroize();

    let nonce = XNonce::from_slice(nonce_bytes);
    let mut plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed: wrong passphrase or corrupt data"))?;

    if plaintext.len() != 32 {
        plaintext.zeroize();
        bail!("decrypted payload has unexpected length");
    }

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&plaintext);
    plaintext.zeroize();

    let identity = NodeIdentity::from_signing_key_bytes(&key_bytes);
    key_bytes.zeroize();
    Ok(identity)
}

fn derive_key(passphrase: &str, salt: &[u8], out: &mut [u8; 32]) {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .expect("argon2 Params are always valid with these constants");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, out)
        .expect("argon2 hash_password_into failed");
}

/// Return the longest prefix of `s` that is at most `max_bytes` long, cut on a
/// UTF-8 char boundary so the result is always valid UTF-8.
fn truncate_to_byte_len(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_display_name() {
        let original = NodeIdentity::generate();
        let bundle = IdentityExport::create(&original, Some("Alice"), "correct-pass").unwrap();
        let (recovered, name) = IdentityExport::load(&bundle, "correct-pass").unwrap();

        assert_eq!(
            original.verifying_key().as_bytes(),
            recovered.verifying_key().as_bytes()
        );
        assert_eq!(name.as_deref(), Some("Alice"));
    }

    #[test]
    fn roundtrip_without_display_name() {
        let original = NodeIdentity::generate();
        let bundle = IdentityExport::create(&original, None, "pass").unwrap();
        let (recovered, name) = IdentityExport::load(&bundle, "pass").unwrap();

        assert_eq!(
            original.verifying_key().as_bytes(),
            recovered.verifying_key().as_bytes()
        );
        assert!(name.is_none());
    }

    #[test]
    fn wrong_passphrase_fails() {
        let identity = NodeIdentity::generate();
        let bundle = IdentityExport::create(&identity, Some("Bob"), "correct").unwrap();
        assert!(IdentityExport::load(&bundle, "wrong").is_err());
    }

    #[test]
    fn corrupt_data_fails() {
        let identity = NodeIdentity::generate();
        let mut bundle = IdentityExport::create(&identity, None, "pass").unwrap();
        let last = bundle.len() - 1;
        bundle[last] ^= 0xFF;
        assert!(IdentityExport::load(&bundle, "pass").is_err());
    }

    #[test]
    fn wrong_magic_fails() {
        let bytes = b"NOPE\x01\x00".to_vec();
        assert!(IdentityExport::load(&bytes, "pass").is_err());
    }

    #[test]
    fn long_multibyte_name_is_truncated_on_char_boundary() {
        let original = NodeIdentity::generate();
        // 32 four-byte emoji = 128 bytes, well over the 64-byte cap.
        let long_name = "😀".repeat(32);
        let bundle = IdentityExport::create(&original, Some(&long_name), "pass").unwrap();
        let (recovered, name) = IdentityExport::load(&bundle, "pass").unwrap();

        assert_eq!(
            original.verifying_key().as_bytes(),
            recovered.verifying_key().as_bytes()
        );
        let name = name.expect("name should be present");
        assert!(name.len() <= 64, "stored name must fit in 64 bytes");
        // 64 / 4 = 16 whole emoji fit without splitting a char.
        assert_eq!(name, "😀".repeat(16));
    }

    #[test]
    fn wrong_version_fails() {
        let identity = NodeIdentity::generate();
        let mut bundle = IdentityExport::create(&identity, None, "pass").unwrap();
        bundle[4] = 99;
        assert!(IdentityExport::load(&bundle, "pass").is_err());
    }
}
