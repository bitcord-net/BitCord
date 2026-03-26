use anyhow::bail;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use hkdf::Hkdf;
use rand::{RngCore, rngs::OsRng};
use sha2::Sha256;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// HKDF info string for channel-key wrapping — domain-separates from DM KDF.
const CHANNEL_KEY_WRAP_INFO: &[u8] = b"bitcord-channel-key-wrap/v1";

/// A 32-byte symmetric key used to encrypt channel messages with XChaCha20Poly1305.
/// Key bytes are zeroed on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ChannelKey([u8; 32]);

impl ChannelKey {
    /// Generate a new random channel key using the OS CSPRNG.
    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        Self(key)
    }

    /// Wrap existing key bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Raw key bytes. The caller is responsible for zeroizing the result after use.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    // ── Message encryption ────────────────────────────────────────────────────

    /// Encrypt `plaintext`, returning `(nonce, ciphertext)` as separate values.
    ///
    /// Prefer this over [`encrypt`] when the protocol stores nonce and ciphertext
    /// in separate fields (e.g. [`RawMessage`]).
    pub fn encrypt_message(&self, plaintext: &[u8]) -> anyhow::Result<([u8; 24], Vec<u8>)> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.0)
            .map_err(|_| anyhow::anyhow!("ChannelKey has invalid length"))?;
        let mut nonce_bytes = [0u8; 24];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| anyhow::anyhow!("Encryption failed"))?;
        Ok((nonce_bytes, ciphertext))
    }

    /// Decrypt ciphertext produced by [`encrypt_message`].
    pub fn decrypt_message(&self, nonce: &[u8; 24], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.0)
            .map_err(|_| anyhow::anyhow!("ChannelKey has invalid length"))?;
        let nonce = XNonce::from_slice(nonce);
        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("Decryption failed: wrong key or corrupt ciphertext"))
    }

    /// Encrypt `plaintext`, returning `nonce (24 bytes) || ciphertext`.
    ///
    /// Convenience wrapper around [`encrypt_message`] for callers that handle
    /// nonce and ciphertext as a single blob.
    pub fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let (nonce, ciphertext) = self.encrypt_message(plaintext)?;
        let mut result = Vec::with_capacity(24 + ciphertext.len());
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt data produced by [`encrypt`].
    ///
    /// Expects `nonce (24 bytes) || ciphertext`.
    pub fn decrypt(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if data.len() < 24 {
            bail!("Ciphertext is too short to contain a nonce");
        }
        let nonce: [u8; 24] = data[..24].try_into().unwrap();
        self.decrypt_message(&nonce, &data[24..])
    }

    // ── Key distribution ──────────────────────────────────────────────────────

    /// Wrap this channel key for delivery to a member's X25519 public key.
    ///
    /// Used to populate `ChannelManifest.encrypted_channel_key`.
    ///
    /// # Wire format
    /// `ephemeral_pk (32) || nonce (24) || ciphertext (48)` = 104 bytes.
    ///
    /// The ciphertext is the 32-byte channel key encrypted under an
    /// HKDF-derived wrapping key:
    /// ```text
    /// shared      = ephemeral_sk × member_pk
    /// wrapping_key = HKDF-SHA256(ikm=shared, salt=ephemeral_pk, info="bitcord-channel-key-wrap/v1")
    /// ciphertext  = XChaCha20Poly1305(wrapping_key).encrypt(channel_key_bytes)
    /// ```
    pub fn encrypt_for_member(&self, member_x25519_pk: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
        let member_pk = X25519PublicKey::from(*member_x25519_pk);
        let ephem_sk = StaticSecret::random_from_rng(OsRng);
        let ephem_pk = X25519PublicKey::from(&ephem_sk);
        let ephem_pk_bytes = ephem_pk.to_bytes();

        let shared = ephem_sk.diffie_hellman(&member_pk);

        let hk = Hkdf::<Sha256>::new(Some(&ephem_pk_bytes), shared.as_bytes());
        let mut wrapping_key = [0u8; 32];
        hk.expand(CHANNEL_KEY_WRAP_INFO, &mut wrapping_key)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;

        let cipher = XChaCha20Poly1305::new_from_slice(&wrapping_key)
            .map_err(|_| anyhow::anyhow!("wrapping key length invalid"))?;
        wrapping_key.zeroize();

        let mut nonce_bytes = [0u8; 24];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, self.0.as_slice())
            .map_err(|_| anyhow::anyhow!("channel key wrapping failed"))?;

        let mut out = Vec::with_capacity(32 + 24 + ciphertext.len());
        out.extend_from_slice(&ephem_pk_bytes);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Unwrap a channel key previously encrypted by [`encrypt_for_member`].
    ///
    /// `x25519_sk` must be the StaticSecret corresponding to the `member_x25519_pk`
    /// that was passed to [`encrypt_for_member`].
    pub fn decrypt_for_self(x25519_sk: &StaticSecret, wrapped: &[u8]) -> anyhow::Result<Self> {
        // Minimum: 32 (ephem_pk) + 24 (nonce) + 16 (AEAD tag) = 72 bytes.
        // We expect exactly 32 + 24 + 48 = 104 bytes (32-byte key + 16-byte tag).
        if wrapped.len() < 32 + 24 + 16 {
            bail!("wrapped key is too short ({} bytes)", wrapped.len());
        }
        let ephem_pk_bytes: [u8; 32] = wrapped[..32].try_into().unwrap();
        let nonce_bytes: [u8; 24] = wrapped[32..56].try_into().unwrap();
        let ciphertext = &wrapped[56..];

        let ephem_pk = X25519PublicKey::from(ephem_pk_bytes);
        let shared = x25519_sk.diffie_hellman(&ephem_pk);

        let hk = Hkdf::<Sha256>::new(Some(&ephem_pk_bytes), shared.as_bytes());
        let mut wrapping_key = [0u8; 32];
        hk.expand(CHANNEL_KEY_WRAP_INFO, &mut wrapping_key)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;

        let cipher = XChaCha20Poly1305::new_from_slice(&wrapping_key)
            .map_err(|_| anyhow::anyhow!("wrapping key length invalid"))?;
        wrapping_key.zeroize();

        let nonce = XNonce::from_slice(&nonce_bytes);
        let key_bytes_vec = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("key unwrap failed: wrong key or corrupt data"))?;

        if key_bytes_vec.len() != 32 {
            bail!(
                "unwrapped key has unexpected length {} (expected 32)",
                key_bytes_vec.len()
            );
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&key_bytes_vec);
        Ok(ChannelKey::from_bytes(key_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Message encryption ────────────────────────────────────────────────────

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = ChannelKey::generate();
        let plaintext = b"secret channel message";
        let encrypted = key.encrypt(plaintext).unwrap();
        let decrypted = key.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_message_decrypt_message_roundtrip() {
        let key = ChannelKey::generate();
        let plaintext = b"split nonce/ciphertext test";
        let (nonce, ciphertext) = key.encrypt_message(plaintext).unwrap();
        let decrypted = key.decrypt_message(&nonce, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn different_nonces_produce_different_ciphertexts() {
        let key = ChannelKey::generate();
        let plaintext = b"same message";
        let c1 = key.encrypt(plaintext).unwrap();
        let c2 = key.encrypt(plaintext).unwrap();
        assert_ne!(c1, c2);
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let key1 = ChannelKey::generate();
        let key2 = ChannelKey::generate();
        let encrypted = key1.encrypt(b"payload").unwrap();
        assert!(key2.decrypt(&encrypted).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails_authentication() {
        let key = ChannelKey::generate();
        let mut encrypted = key.encrypt(b"authentic message").unwrap();
        encrypted[30] ^= 0xFF;
        assert!(key.decrypt(&encrypted).is_err());
    }

    #[test]
    fn truncated_data_is_rejected() {
        let key = ChannelKey::generate();
        assert!(key.decrypt(&[0u8; 10]).is_err());
    }

    // ── Key distribution ──────────────────────────────────────────────────────

    #[test]
    fn encrypt_for_member_decrypt_for_self_roundtrip() {
        let channel_key = ChannelKey::generate();
        let member_sk = StaticSecret::random_from_rng(OsRng);
        let member_pk = X25519PublicKey::from(&member_sk).to_bytes();

        let wrapped = channel_key.encrypt_for_member(&member_pk).unwrap();
        let recovered = ChannelKey::decrypt_for_self(&member_sk, &wrapped).unwrap();

        assert_eq!(channel_key.as_bytes(), recovered.as_bytes());
    }

    #[test]
    fn wrong_member_key_fails_unwrap() {
        let channel_key = ChannelKey::generate();
        let member_sk = StaticSecret::random_from_rng(OsRng);
        let member_pk = X25519PublicKey::from(&member_sk).to_bytes();
        let wrong_sk = StaticSecret::random_from_rng(OsRng);

        let wrapped = channel_key.encrypt_for_member(&member_pk).unwrap();
        assert!(ChannelKey::decrypt_for_self(&wrong_sk, &wrapped).is_err());
    }

    #[test]
    fn different_wrappings_for_same_key() {
        let channel_key = ChannelKey::generate();
        let member_sk = StaticSecret::random_from_rng(OsRng);
        let member_pk = X25519PublicKey::from(&member_sk).to_bytes();

        let w1 = channel_key.encrypt_for_member(&member_pk).unwrap();
        let w2 = channel_key.encrypt_for_member(&member_pk).unwrap();
        // Each wrapping uses a fresh ephemeral key — blobs must differ.
        assert_ne!(w1, w2);
        // But both decrypt to the same channel key.
        let r1 = ChannelKey::decrypt_for_self(&member_sk, &w1).unwrap();
        let r2 = ChannelKey::decrypt_for_self(&member_sk, &w2).unwrap();
        assert_eq!(r1.as_bytes(), r2.as_bytes());
    }

    #[test]
    fn truncated_wrapped_key_rejected() {
        let member_sk = StaticSecret::random_from_rng(OsRng);
        assert!(ChannelKey::decrypt_for_self(&member_sk, &[0u8; 20]).is_err());
    }
}
