use std::path::Path;

use anyhow::{Context, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use rand::{RngCore, rngs::OsRng};
use zeroize::Zeroize;

use super::NodeIdentity;

/// Argon2id parameters for key derivation.
///
/// Memory: 64 MiB, 3 iterations, 4 lanes — appropriate for a desktop app.
const ARGON2_M_COST: u32 = 65_536; // KiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;

/// On-disk layout (all fields concatenated, no framing):
/// - bytes  0..32 : random 32-byte salt
/// - bytes 32..56 : random 24-byte XChaCha20Poly1305 nonce
/// - bytes 56..   : ciphertext = encrypt(signing_key_bytes[32]) → 48 bytes (32 + 16-byte tag)
///
/// Total: 104 bytes.
pub struct KeyStore;

impl KeyStore {
    /// Persist `identity` to `path`, encrypted with `passphrase`.
    pub fn save(path: &Path, identity: &NodeIdentity, passphrase: &str) -> anyhow::Result<()> {
        let mut salt = [0u8; 32];
        OsRng.fill_bytes(&mut salt);

        let mut nonce_bytes = [0u8; 24];
        OsRng.fill_bytes(&mut nonce_bytes);

        let mut okm = [0u8; 32];
        derive_key(passphrase, &salt, &mut okm);

        let cipher = XChaCha20Poly1305::new_from_slice(&okm)
            .context("Failed to create cipher (key length mismatch)")?;
        okm.zeroize();

        let nonce = XNonce::from_slice(&nonce_bytes);
        let signing_bytes = identity.signing_key_bytes();
        let ciphertext = cipher
            .encrypt(nonce, signing_bytes.as_ref())
            .map_err(|_| anyhow::anyhow!("Encryption failed"))?;

        let mut file_bytes = Vec::with_capacity(32 + 24 + ciphertext.len());
        file_bytes.extend_from_slice(&salt);
        file_bytes.extend_from_slice(&nonce_bytes);
        file_bytes.extend_from_slice(&ciphertext);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create keystore directory")?;
        }
        std::fs::write(path, &file_bytes).context("Failed to write keystore file")?;
        Ok(())
    }

    /// Load an identity from `path`, decrypting with `passphrase`.
    /// Returns an error if the passphrase is wrong or the file is corrupt.
    pub fn load(path: &Path, passphrase: &str) -> anyhow::Result<NodeIdentity> {
        let data = std::fs::read(path).context("Failed to read keystore file")?;

        if data.len() < 56 {
            bail!("Keystore file is too short to be valid");
        }

        let salt = &data[0..32];
        let nonce_bytes = &data[32..56];
        let ciphertext = &data[56..];

        let mut okm = [0u8; 32];
        derive_key(passphrase, salt, &mut okm);

        let cipher = XChaCha20Poly1305::new_from_slice(&okm).context("Failed to create cipher")?;
        okm.zeroize();

        let nonce = XNonce::from_slice(nonce_bytes);
        let mut plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("Decryption failed: wrong passphrase or corrupt file"))?;

        if plaintext.len() != 32 {
            plaintext.zeroize();
            bail!("Decrypted payload has unexpected length");
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&plaintext);
        plaintext.zeroize();

        let identity = NodeIdentity::from_signing_key_bytes(&key_bytes);
        key_bytes.zeroize();
        Ok(identity)
    }
}

/// Derive a 32-byte key from `passphrase` and `salt` using Argon2id.
///
/// Uses conservative parameters (64 MiB / 3 iterations / 4 lanes) suitable
/// for a desktop application where key stretching is done once at unlock time.
fn derive_key(passphrase: &str, salt: &[u8], out: &mut [u8; 32]) {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .expect("argon2 Params are always valid with these constants");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, out)
        .expect("argon2 hash_password_into failed (salt too short?)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn save_and_load_correct_passphrase() {
        let original = NodeIdentity::generate();
        let file = NamedTempFile::new().unwrap();

        KeyStore::save(file.path(), &original, "correct-passphrase").unwrap();
        let loaded = KeyStore::load(file.path(), "correct-passphrase").unwrap();

        assert_eq!(
            original.verifying_key().as_bytes(),
            loaded.verifying_key().as_bytes()
        );
        assert_eq!(original.to_peer_id(), loaded.to_peer_id());
    }

    #[test]
    fn load_wrong_passphrase_fails() {
        let identity = NodeIdentity::generate();
        let file = NamedTempFile::new().unwrap();

        KeyStore::save(file.path(), &identity, "correct").unwrap();
        let result = KeyStore::load(file.path(), "wrong");
        assert!(result.is_err());
    }

    #[test]
    fn load_corrupt_file_fails() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"this is not a valid keystore").unwrap();
        assert!(KeyStore::load(file.path(), "any").is_err());
    }
}
