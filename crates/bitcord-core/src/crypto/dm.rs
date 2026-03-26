//! Encrypted direct-message envelopes.
//!
//! Uses X25519 ephemeral key agreement + HKDF-SHA256 + XChaCha20-Poly1305.
//! Both an ephemeral key and the sender's static X25519 key contribute to the
//! derived encryption key, providing forward secrecy and implicit sender
//! authentication.

use anyhow::Result;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use hkdf::Hkdf;
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

/// HKDF info string — domain-separates this key derivation from all others.
const DM_HKDF_INFO: &[u8] = b"bitcord-dm/v1";

/// Structured plaintext sealed inside a [`DmEnvelope`].
///
/// Serialised with `postcard`.  The receive path falls back to treating the
/// plaintext as raw UTF-8 for envelopes produced by older clients that only
/// sealed the message body.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct DmPayload {
    pub body: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Sender-assigned message ID (ULID).  Carried in the payload so both
    /// parties store the message under the same canonical ID, which is
    /// required for reply-quote lookups to work on the recipient's side.
    /// Empty string means "no ID in payload" (pre-v2 envelopes).
    #[serde(default)]
    pub id: String,
}

/// An encrypted direct-message envelope.
///
/// # Key derivation
/// ```text
/// dh1 = ephemeral_sk  × recipient_pk     (fresh per-message ephemeral)
/// dh2 = sender_sk     × recipient_pk     (sender's static X25519 key)
/// ikm = dh1 || dh2
/// key = HKDF-SHA256(ikm, salt=ephemeral_pk, info="bitcord-dm/v1")
/// ```
///
/// The recipient mirrors the computation using their own static key:
/// ```text
/// dh1 = recipient_sk × ephemeral_pk
/// dh2 = recipient_sk × sender_pk
/// ```
///
/// This two-sided contribution means:
/// - A passive observer who later learns `sender_sk` cannot decrypt old
///   messages (forward secrecy via the per-message ephemeral).
/// - A recipient who doesn't know `sender_sk` cannot produce a valid
///   ciphertext that opens (implicit sender authentication).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DmEnvelope {
    /// Sender's static X25519 public key bytes.
    pub sender_pk: [u8; 32],
    /// Per-message ephemeral X25519 public key bytes.
    pub ephem_pk: [u8; 32],
    /// XChaCha20-Poly1305 nonce (24 bytes, randomly generated).
    pub nonce: [u8; 24],
    /// Authenticated ciphertext (`plaintext.len() + 16` bytes).
    pub ciphertext: Vec<u8>,
}

impl DmEnvelope {
    /// Seal `plaintext` for `recipient_pk` using the sender's static X25519 key.
    pub fn seal(
        sender_sk: &StaticSecret,
        recipient_pk: &PublicKey,
        plaintext: &[u8],
    ) -> Result<Self> {
        let sender_pk_bytes = PublicKey::from(sender_sk).to_bytes();

        let ephem_sk = StaticSecret::random_from_rng(OsRng);
        let ephem_pk_bytes = PublicKey::from(&ephem_sk).to_bytes();

        let dh_ephem = ephem_sk.diffie_hellman(recipient_pk);
        let dh_static = sender_sk.diffie_hellman(recipient_pk);

        let mut ikm = [0u8; 64];
        ikm[..32].copy_from_slice(dh_ephem.as_bytes());
        ikm[32..].copy_from_slice(dh_static.as_bytes());

        let hk = Hkdf::<Sha256>::new(Some(&ephem_pk_bytes), &ikm);
        let mut enc_key = [0u8; 32];
        hk.expand(DM_HKDF_INFO, &mut enc_key)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
        ikm.zeroize();

        let cipher = XChaCha20Poly1305::new_from_slice(&enc_key)
            .map_err(|_| anyhow::anyhow!("derived key has invalid length"))?;
        enc_key.zeroize();

        let mut nonce_bytes = [0u8; 24];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| anyhow::anyhow!("DM encryption failed"))?;

        Ok(Self {
            sender_pk: sender_pk_bytes,
            ephem_pk: ephem_pk_bytes,
            nonce: nonce_bytes,
            ciphertext,
        })
    }

    /// Open this envelope using the recipient's static X25519 key.
    ///
    /// Returns the plaintext on success, or an error if the ciphertext has
    /// been tampered with or the wrong key is supplied.
    pub fn open(&self, recipient_sk: &StaticSecret) -> Result<Vec<u8>> {
        let ephem_pk = PublicKey::from(self.ephem_pk);
        let sender_pk = PublicKey::from(self.sender_pk);

        let dh_ephem = recipient_sk.diffie_hellman(&ephem_pk);
        let dh_static = recipient_sk.diffie_hellman(&sender_pk);

        let mut ikm = [0u8; 64];
        ikm[..32].copy_from_slice(dh_ephem.as_bytes());
        ikm[32..].copy_from_slice(dh_static.as_bytes());

        let hk = Hkdf::<Sha256>::new(Some(&self.ephem_pk), &ikm);
        let mut enc_key = [0u8; 32];
        hk.expand(DM_HKDF_INFO, &mut enc_key)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
        ikm.zeroize();

        let cipher = XChaCha20Poly1305::new_from_slice(&enc_key)
            .map_err(|_| anyhow::anyhow!("derived key has invalid length"))?;
        enc_key.zeroize();

        let nonce = XNonce::from_slice(&self.nonce);
        cipher
            .decrypt(nonce, self.ciphertext.as_slice())
            .map_err(|_| anyhow::anyhow!("DM decryption failed: wrong key or tampered ciphertext"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> StaticSecret {
        StaticSecret::random_from_rng(OsRng)
    }

    #[test]
    fn seal_open_roundtrip() {
        let sender_sk = keypair();
        let recipient_sk = keypair();
        let recipient_pk = PublicKey::from(&recipient_sk);
        let plaintext = b"hello, encrypted world!";

        let envelope = DmEnvelope::seal(&sender_sk, &recipient_pk, plaintext).unwrap();
        let decrypted = envelope.open(&recipient_sk).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_recipient_key_fails() {
        let sender_sk = keypair();
        let recipient_sk = keypair();
        let recipient_pk = PublicKey::from(&recipient_sk);
        let wrong_sk = keypair();

        let envelope = DmEnvelope::seal(&sender_sk, &recipient_pk, b"secret").unwrap();
        assert!(envelope.open(&wrong_sk).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let sender_sk = keypair();
        let recipient_sk = keypair();
        let recipient_pk = PublicKey::from(&recipient_sk);

        let mut envelope = DmEnvelope::seal(&sender_sk, &recipient_pk, b"authentic").unwrap();
        envelope.ciphertext[0] ^= 0xFF;
        assert!(envelope.open(&recipient_sk).is_err());
    }

    #[test]
    fn same_plaintext_different_nonces() {
        let sender_sk = keypair();
        let recipient_sk = keypair();
        let recipient_pk = PublicKey::from(&recipient_sk);
        let plaintext = b"same message";

        let e1 = DmEnvelope::seal(&sender_sk, &recipient_pk, plaintext).unwrap();
        let e2 = DmEnvelope::seal(&sender_sk, &recipient_pk, plaintext).unwrap();
        // Nonces are random; ciphertexts must differ.
        assert_ne!(e1.ciphertext, e2.ciphertext);
        assert_ne!(e1.nonce, e2.nonce);
    }

    #[test]
    fn wrong_sender_key_attribution_fails() {
        // If an attacker crafts a DM claiming to be from a different sender,
        // decryption fails because dh_static uses the attacker's real key,
        // not the claimed sender_pk.
        let real_sender_sk = keypair();
        let attacker_sk = keypair();
        let recipient_sk = keypair();
        let recipient_pk = PublicKey::from(&recipient_sk);

        let mut envelope = DmEnvelope::seal(&real_sender_sk, &recipient_pk, b"real").unwrap();
        // Attacker replaces sender_pk with their own public key — dh_static now mismatches.
        envelope.sender_pk = PublicKey::from(&attacker_sk).to_bytes();
        assert!(envelope.open(&recipient_sk).is_err());
    }
}
