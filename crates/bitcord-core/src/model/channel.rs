use crate::model::types::{ChannelId, CommunityId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The type/purpose of a channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    Text,
    Announcement,
    Voice,
}

/// Channel manifest stored in the DHT and distributed to members.
///
/// The `encrypted_channel_key` map holds the 32-byte `ChannelKey` encrypted
/// with each member's X25519 public key (via ECDH + XChaCha20-Poly1305).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelManifest {
    pub id: ChannelId,
    pub community_id: CommunityId,
    pub name: String,
    pub kind: ChannelKind,
    /// Per-member encrypted channel key: `UserId` → ciphertext.
    pub encrypted_channel_key: HashMap<UserId, Vec<u8>>,
    pub created_at: DateTime<Utc>,
    /// Monotonically-increasing version; increment on key rotation or metadata changes.
    pub version: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_json() {
        let uid = UserId([42u8; 32]);
        let manifest = ChannelManifest {
            id: ChannelId::new(),
            community_id: CommunityId::new(),
            name: "general".into(),
            kind: ChannelKind::Text,
            encrypted_channel_key: HashMap::from([(uid, vec![1, 2, 3])]),
            created_at: Utc::now(),
            version: 1,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let restored: ChannelManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest.name, restored.name);
        assert_eq!(manifest.kind, restored.kind);
        assert_eq!(restored.encrypted_channel_key.len(), 1);
    }

    #[test]
    fn serde_roundtrip_postcard() {
        let manifest = ChannelManifest {
            id: ChannelId::new(),
            community_id: CommunityId::new(),
            name: "announcements".into(),
            kind: ChannelKind::Announcement,
            encrypted_channel_key: HashMap::new(),
            created_at: Utc::now(),
            version: 1,
        };
        let bytes = postcard::to_allocvec(&manifest).unwrap();
        let restored: ChannelManifest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(manifest.name, restored.name);
    }
}
