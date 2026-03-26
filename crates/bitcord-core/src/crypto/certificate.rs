//! Hosting certificates — proof that a community admin authorised a node to host their community.

use anyhow::{Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Ed25519-signed certificate granting a node the right to host a community.
///
/// Issued by the community admin (holder of `community_sk`) and presented by
/// the node when answering `ClientRequest::JoinCommunity`.
///
/// # Security
/// A certificate is only valid if:
/// 1. The Ed25519 signature verifies against `community_pk`, and
/// 2. The current wall-clock time is ≤ `expires_at`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostingCert {
    /// Community's Ed25519 public key (verifying key bytes).
    pub community_pk: [u8; 32],
    /// Node's Ed25519 public key (verifying key bytes).
    pub node_pk: [u8; 32],
    /// Unix timestamp (seconds) after which this certificate is no longer valid.
    pub expires_at: u64,
    /// Ed25519 signature over `community_pk || node_pk || expires_at.to_le_bytes()`.
    /// Uses `ed25519_dalek::Signature` for serde compatibility.
    pub signature: Signature,
}

impl HostingCert {
    /// Canonical byte sequence that is signed and verified.
    ///
    /// Layout: `community_pk (32) || node_pk (32) || expires_at LE (8)` = 72 bytes.
    fn signable(community_pk: &[u8; 32], node_pk: &[u8; 32], expires_at: u64) -> [u8; 72] {
        let mut buf = [0u8; 72];
        buf[..32].copy_from_slice(community_pk);
        buf[32..64].copy_from_slice(node_pk);
        buf[64..].copy_from_slice(&expires_at.to_le_bytes());
        buf
    }

    /// Issue a new hosting certificate signed by the community admin.
    ///
    /// # Arguments
    /// * `community_sk` — the admin's Ed25519 signing key
    /// * `node_pk`      — raw bytes of the node's Ed25519 public key
    /// * `expires_at`   — Unix timestamp (seconds) when this cert expires
    pub fn new(community_sk: &SigningKey, node_pk: [u8; 32], expires_at: u64) -> Self {
        let community_pk = community_sk.verifying_key().to_bytes();
        let msg = Self::signable(&community_pk, &node_pk, expires_at);
        let signature = community_sk.sign(&msg);
        Self {
            community_pk,
            node_pk,
            expires_at,
            signature,
        }
    }

    /// Verify this certificate against the expected community public key.
    ///
    /// Returns `Ok(())` if the signature is valid and the cert has not expired.
    ///
    /// # Errors
    /// - community key mismatch
    /// - Ed25519 signature verification failure
    /// - certificate is expired
    pub fn verify(&self, community_pk: &VerifyingKey) -> Result<()> {
        if community_pk.to_bytes() != self.community_pk {
            bail!("HostingCert community key mismatch");
        }
        let msg = Self::signable(&self.community_pk, &self.node_pk, self.expires_at);
        community_pk
            .verify(&msg, &self.signature)
            .map_err(|_| anyhow::anyhow!("HostingCert signature invalid"))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now > self.expires_at {
            bail!(
                "HostingCert expired (expires_at={}, now={})",
                self.expires_at,
                now
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn fresh_signing_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn far_future() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86_400 * 365 // one year from now
    }

    #[test]
    fn issue_and_verify_roundtrip() {
        let community_sk = fresh_signing_key();
        let node_sk = fresh_signing_key();
        let node_pk = node_sk.verifying_key().to_bytes();
        let cert = HostingCert::new(&community_sk, node_pk, far_future());
        assert!(cert.verify(&community_sk.verifying_key()).is_ok());
    }

    #[test]
    fn wrong_community_key_rejected() {
        let community_sk = fresh_signing_key();
        let wrong_sk = fresh_signing_key();
        let node_pk = fresh_signing_key().verifying_key().to_bytes();
        let cert = HostingCert::new(&community_sk, node_pk, far_future());
        assert!(cert.verify(&wrong_sk.verifying_key()).is_err());
    }

    #[test]
    fn expired_cert_rejected() {
        let community_sk = fresh_signing_key();
        let node_pk = fresh_signing_key().verifying_key().to_bytes();
        // expires_at = 1 (1970-01-01T00:00:01Z) — always in the past
        let cert = HostingCert::new(&community_sk, node_pk, 1);
        assert!(cert.verify(&community_sk.verifying_key()).is_err());
    }

    #[test]
    fn tampered_node_pk_rejected() {
        let community_sk = fresh_signing_key();
        let node_pk = fresh_signing_key().verifying_key().to_bytes();
        let mut cert = HostingCert::new(&community_sk, node_pk, far_future());
        cert.node_pk[0] ^= 0xFF; // flip a bit
        assert!(cert.verify(&community_sk.verifying_key()).is_err());
    }

    #[test]
    fn tampered_expiry_rejected() {
        let community_sk = fresh_signing_key();
        let node_pk = fresh_signing_key().verifying_key().to_bytes();
        let mut cert = HostingCert::new(&community_sk, node_pk, far_future());
        cert.expires_at += 1; // extend expiry without re-signing
        assert!(cert.verify(&community_sk.verifying_key()).is_err());
    }

    #[test]
    fn tampered_signature_rejected() {
        let community_sk = fresh_signing_key();
        let node_pk = fresh_signing_key().verifying_key().to_bytes();
        let mut cert = HostingCert::new(&community_sk, node_pk, far_future());
        // Flip a byte in the signature bytes to break verification.
        let mut sig_bytes = cert.signature.to_bytes();
        sig_bytes[0] ^= 0xFF;
        cert.signature = Signature::from_bytes(&sig_bytes);
        assert!(cert.verify(&community_sk.verifying_key()).is_err());
    }
}
