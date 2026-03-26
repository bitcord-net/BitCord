use crate::model::types::{CommunityId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A named role within a community.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Admin,
    Moderator,
    Member,
}

/// Membership record published to the community DHT key.
///
/// Contains both the Ed25519 public key (for signature verification) and the
/// X25519 public key (for encrypting channel keys to this member).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MembershipRecord {
    pub user_id: UserId,
    pub community_id: CommunityId,
    pub display_name: String,
    /// Optional IPFS CID or URL for the user's avatar.
    pub avatar_cid: Option<String>,
    pub joined_at: DateTime<Utc>,
    pub roles: Vec<Role>,
    /// Ed25519 verifying key bytes (32 bytes).
    pub public_key: [u8; 32],
    /// X25519 public key bytes (32 bytes) for DM/channel key encryption.
    pub x25519_public_key: [u8; 32],
    /// Ed25519 signature over `user_id || community_id || display_name || joined_at_ms_le || postcard(roles)`.
    /// Signed by the joining member's own Ed25519 key.
    pub signature: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_json() {
        let record = MembershipRecord {
            user_id: UserId([1u8; 32]),
            community_id: CommunityId::new(),
            display_name: "Alice".into(),
            avatar_cid: Some("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".into()),
            joined_at: Utc::now(),
            roles: vec![Role::Member],
            public_key: [2u8; 32],
            x25519_public_key: [3u8; 32],
            signature: vec![0u8; 64],
        };
        let json = serde_json::to_string(&record).unwrap();
        let restored: MembershipRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record.display_name, restored.display_name);
        assert_eq!(record.roles, restored.roles);
        assert_eq!(record.public_key, restored.public_key);
        assert_eq!(record.x25519_public_key, restored.x25519_public_key);
    }

    #[test]
    fn serde_roundtrip_postcard() {
        let record = MembershipRecord {
            user_id: UserId([0u8; 32]),
            community_id: CommunityId::new(),
            display_name: "Bob".into(),
            avatar_cid: None,
            joined_at: Utc::now(),
            roles: vec![Role::Admin, Role::Moderator],
            public_key: [4u8; 32],
            x25519_public_key: [5u8; 32],
            signature: vec![0u8; 64],
        };
        let bytes = postcard::to_allocvec(&record).unwrap();
        let restored: MembershipRecord = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(record.display_name, restored.display_name);
        assert_eq!(record.roles, restored.roles);
    }
}
