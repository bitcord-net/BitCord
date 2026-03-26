use ed25519_dalek::{Signer, Verifier};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::fmt;
use x25519_dalek::StaticSecret;
use zeroize::Zeroize;

pub mod keystore;

pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

/// A 32-byte SHA-256 digest of the verifying key, used as a peer identifier.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PeerId([u8; 32]);

impl PeerId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", self)
    }
}

/// Holds an Ed25519 signing key and its derived verifying key.
/// The signing key bytes are zeroed on drop via `ed25519-dalek`'s `ZeroizeOnDrop`.
pub struct NodeIdentity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl NodeIdentity {
    /// Generate a new random identity using the OS CSPRNG.
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
        }
    }

    /// Reconstruct an identity from raw signing-key bytes.
    pub fn from_signing_key_bytes(bytes: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(bytes);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
        }
    }

    /// Derive the peer ID as SHA-256(verifying_key_bytes).
    pub fn to_peer_id(&self) -> PeerId {
        let hash = Sha256::digest(self.verifying_key.as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash);
        PeerId(bytes)
    }

    /// Sign a message. The signature covers the raw bytes as-is.
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.signing_key.sign(msg)
    }

    /// Return a reference to the verifying (public) key.
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    /// Return a clone of the signing key.
    pub fn signing_key(&self) -> SigningKey {
        self.signing_key.clone()
    }

    /// Return the raw signing-key bytes. Used only by the keystore serialiser.
    /// The caller is responsible for zeroizing the returned array after use.
    pub fn signing_key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// Derive a deterministic X25519 static secret from the Ed25519 signing key.
    ///
    /// Used for DM encryption. The secret is derived by hashing the
    /// Ed25519 signing key bytes with SHA-256; x25519-dalek handles the
    /// Curve25519 scalar clamping internally.
    pub fn x25519_secret(&self) -> StaticSecret {
        let hash = Sha256::digest(self.signing_key.to_bytes());
        let scalar: [u8; 32] = hash.into();
        StaticSecret::from(scalar)
    }

    /// Return the X25519 public key bytes derived from this identity's signing key.
    ///
    /// This is the value that should be stored in `MembershipRecord.x25519_public_key`
    /// so that other members can encrypt channel keys to this node.
    pub fn x25519_public_key_bytes(&self) -> [u8; 32] {
        x25519_dalek::PublicKey::from(&self.x25519_secret()).to_bytes()
    }

    /// Return the node's canonical network address: Base58-encoded Ed25519 public key bytes.
    ///
    /// This is the address used in invite links and displayed to users.
    /// Format: `bs58(verifying_key.as_bytes())` — 44 characters, no prefix.
    pub fn node_address(&self) -> String {
        bs58::encode(self.verifying_key.as_bytes()).into_string()
    }
}

impl Zeroize for NodeIdentity {
    fn zeroize(&mut self) {
        // `SigningKey` does not impl `Zeroize` directly in ed25519-dalek 2.x, but it
        // does have `ZeroizeOnDrop`. Assigning a zeroed key drops the old value,
        // triggering its `ZeroizeOnDrop` and wiping the original key bytes from memory.
        let zero_bytes = [0u8; 32];
        self.signing_key = SigningKey::from_bytes(&zero_bytes);
        self.verifying_key = self.signing_key.verifying_key();
    }
}

impl Drop for NodeIdentity {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Verify an Ed25519 signature produced by `NodeIdentity::sign`.
pub fn verify(pubkey: &VerifyingKey, msg: &[u8], sig: &Signature) -> bool {
    pubkey.verify(msg, sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_unique() {
        let a = NodeIdentity::generate();
        let b = NodeIdentity::generate();
        assert_ne!(a.verifying_key().as_bytes(), b.verifying_key().as_bytes());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let identity = NodeIdentity::generate();
        let msg = b"hello bitcord";
        let sig = identity.sign(msg);
        assert!(verify(identity.verifying_key(), msg, &sig));
    }

    #[test]
    fn verify_rejects_tampered_message() {
        let identity = NodeIdentity::generate();
        let sig = identity.sign(b"original");
        assert!(!verify(identity.verifying_key(), b"tampered", &sig));
    }

    #[test]
    fn peer_id_is_deterministic() {
        let identity = NodeIdentity::generate();
        assert_eq!(identity.to_peer_id(), identity.to_peer_id());
    }

    #[test]
    fn peer_id_differs_across_identities() {
        let a = NodeIdentity::generate();
        let b = NodeIdentity::generate();
        assert_ne!(a.to_peer_id(), b.to_peer_id());
    }

    #[test]
    fn roundtrip_from_signing_key_bytes() {
        let original = NodeIdentity::generate();
        let bytes = original.signing_key_bytes();
        let restored = NodeIdentity::from_signing_key_bytes(&bytes);
        assert_eq!(
            original.verifying_key().as_bytes(),
            restored.verifying_key().as_bytes()
        );
        assert_eq!(original.to_peer_id(), restored.to_peer_id());
    }
}
