use crate::model::types::{ChannelId, CommunityId, UserId};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The canonical community manifest, signable and publishable to the DHT.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommunityManifest {
    pub id: CommunityId,
    pub name: String,
    pub description: String,
    /// Ed25519 public key of the founding/primary admin (32 bytes).
    pub public_key: [u8; 32],
    pub created_at: DateTime<Utc>,
    pub admin_ids: Vec<UserId>,
    pub channel_ids: Vec<ChannelId>,
    /// Multiaddresses of known seed nodes for this community.
    pub seed_nodes: Vec<String>,
    /// Monotonically-increasing version; increment on every update.
    pub version: u64,
    /// Set to `true` when an admin permanently deletes the community.
    /// Receiving a `ManifestUpdate` with `deleted = true` causes members to
    /// remove the community from their local state.
    #[serde(default)]
    pub deleted: bool,
}

impl CommunityManifest {
    /// Serialize the manifest to canonical bytes for signing/verification.
    fn canonical_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("CommunityManifest serialization is infallible")
    }

    /// Sign this manifest with the given admin key and return a `SignedManifest`.
    pub fn sign(self, admin_key: &SigningKey) -> SignedManifest {
        let bytes = self.canonical_bytes();
        let signature = admin_key.sign(&bytes);
        SignedManifest {
            manifest: self,
            signature: signature.to_bytes().to_vec(),
        }
    }
}

/// A `CommunityManifest` with an Ed25519 signature from the admin key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedManifest {
    pub manifest: CommunityManifest,
    /// Ed25519 signature bytes (64 bytes).
    pub signature: Vec<u8>,
}

impl SignedManifest {
    /// Verify the signature against `manifest.public_key`.
    /// Returns `false` if the key or signature bytes are malformed.
    pub fn verify(&self) -> bool {
        let key_bytes: [u8; 32] = self.manifest.public_key;
        let Ok(vk) = VerifyingKey::from_bytes(&key_bytes) else {
            return false;
        };
        let Ok(sig_bytes): Result<[u8; 64], _> = self.signature.as_slice().try_into() else {
            return false;
        };
        let sig = Signature::from_bytes(&sig_bytes);
        let bytes = self.manifest.canonical_bytes();
        vk.verify(&bytes, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::UserId;
    use rand::rngs::OsRng;

    fn make_manifest(admin_key: &SigningKey) -> CommunityManifest {
        let vk = admin_key.verifying_key();
        let admin_id = UserId::from_verifying_key(&vk);
        CommunityManifest {
            id: CommunityId::new(),
            name: "Test Community".into(),
            description: "A community for testing".into(),
            public_key: vk.to_bytes(),
            created_at: Utc::now(),
            admin_ids: vec![admin_id],
            channel_ids: vec![],
            seed_nodes: vec!["127.0.0.1:7332".into()],
            version: 1,
            deleted: false,
        }
    }

    #[test]
    fn sign_and_verify() {
        let admin_key = SigningKey::generate(&mut OsRng);
        let manifest = make_manifest(&admin_key);
        let signed = manifest.sign(&admin_key);
        assert!(signed.verify());
    }

    #[test]
    fn tampered_manifest_fails_verification() {
        let admin_key = SigningKey::generate(&mut OsRng);
        let manifest = make_manifest(&admin_key);
        let mut signed = manifest.sign(&admin_key);
        signed.manifest.name = "Evil Community".into();
        assert!(!signed.verify());
    }

    #[test]
    fn tampered_signature_fails_verification() {
        let admin_key = SigningKey::generate(&mut OsRng);
        let manifest = make_manifest(&admin_key);
        let mut signed = manifest.sign(&admin_key);
        signed.signature[0] ^= 0xFF;
        assert!(!signed.verify());
    }

    #[test]
    fn serde_roundtrip_json() {
        let admin_key = SigningKey::generate(&mut OsRng);
        let signed = make_manifest(&admin_key).sign(&admin_key);
        let json = serde_json::to_string(&signed).unwrap();
        let restored: SignedManifest = serde_json::from_str(&json).unwrap();
        assert!(restored.verify());
        assert_eq!(signed.manifest.name, restored.manifest.name);
    }

    #[test]
    fn serde_roundtrip_postcard() {
        let admin_key = SigningKey::generate(&mut OsRng);
        let signed = make_manifest(&admin_key).sign(&admin_key);
        let bytes = postcard::to_allocvec(&signed).unwrap();
        let restored: SignedManifest = postcard::from_bytes(&bytes).unwrap();
        assert!(restored.verify());
    }
}
