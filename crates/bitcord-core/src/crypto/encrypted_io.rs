use anyhow::{Context, bail};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use rand::{RngCore, rngs::OsRng};

/// Encrypt `data` with XChaCha20-Poly1305.
///
/// Returns `nonce(24) || ciphertext(len + 16)`.
pub fn encrypt_bytes(data: &[u8], key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).context("invalid encryption key length")?;

    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut out = Vec::with_capacity(24 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a blob produced by [`encrypt_bytes`].
///
/// Expects `nonce(24) || ciphertext(...)`.
pub fn decrypt_bytes(blob: &[u8], key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    if blob.len() < 24 {
        bail!("encrypted blob too short (need at least 24-byte nonce)");
    }

    let nonce = XNonce::from_slice(&blob[..24]);
    let ciphertext = &blob[24..];

    let cipher = XChaCha20Poly1305::new_from_slice(key).context("invalid encryption key length")?;

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed: wrong key or corrupt data"))
}

/// Derive a 32-byte table-encryption key from `passphrase` and `salt` via
/// Argon2id (same parameters as the identity keystore).
pub fn derive_table_key(passphrase: &str, salt: &[u8; 32]) -> [u8; 32] {
    use argon2::{Algorithm, Argon2, Params, Version};

    let params = Params::new(65_536, 3, 4, Some(32)).expect("valid argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .expect("argon2 hash_password_into failed");
    key
}

/// Load or create the table-encryption salt file at `path`.
///
/// If the file exists, reads and returns the 32-byte salt.
/// Otherwise generates a random salt, writes it to `path`, and returns it.
pub fn load_or_create_salt(path: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    if path.exists() {
        let data = std::fs::read(path).context("read table salt file")?;
        if data.len() != 32 {
            bail!(
                "table salt file has unexpected length {} (expected 32)",
                data.len()
            );
        }
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&data);
        Ok(salt)
    } else {
        let mut salt = [0u8; 32];
        OsRng.fill_bytes(&mut salt);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create salt file directory")?;
        }
        std::fs::write(path, salt).context("write table salt file")?;
        Ok(salt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = [42u8; 32];
        let plaintext = b"hello, encrypted world!";
        let blob = encrypt_bytes(plaintext, &key).unwrap();
        let decrypted = decrypt_bytes(&blob, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key = [1u8; 32];
        let wrong = [2u8; 32];
        let blob = encrypt_bytes(b"secret", &key).unwrap();
        assert!(decrypt_bytes(&blob, &wrong).is_err());
    }

    #[test]
    fn truncated_blob_fails() {
        let key = [1u8; 32];
        assert!(decrypt_bytes(&[0u8; 10], &key).is_err());
    }

    #[test]
    fn empty_plaintext_round_trip() {
        let key = [99u8; 32];
        let blob = encrypt_bytes(b"", &key).unwrap();
        let decrypted = decrypt_bytes(&blob, &key).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn derive_table_key_deterministic() {
        let salt = [7u8; 32];
        let k1 = derive_table_key("my-passphrase", &salt);
        let k2 = derive_table_key("my-passphrase", &salt);
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_table_key_different_passphrase() {
        let salt = [7u8; 32];
        let k1 = derive_table_key("pass-a", &salt);
        let k2 = derive_table_key("pass-b", &salt);
        assert_ne!(k1, k2);
    }

    #[test]
    fn load_or_create_salt_creates_and_reloads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("table.salt");
        let s1 = load_or_create_salt(&path).unwrap();
        let s2 = load_or_create_salt(&path).unwrap();
        assert_eq!(s1, s2);
        assert_eq!(std::fs::read(&path).unwrap().len(), 32);
    }
}
