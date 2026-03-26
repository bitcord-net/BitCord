use crate::model::{
    channel::ChannelManifest,
    community::SignedManifest,
    membership::{MembershipRecord, Role},
    message::RawMessage,
    types::{ChannelId, CommunityId, MessageId, UserId},
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Payload for an in-place message edit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditMessagePayload {
    pub message_id: MessageId,
    pub channel_id: ChannelId,
    pub author_id: UserId,
    pub new_ciphertext: Vec<u8>,
    pub new_nonce: [u8; 24],
    /// Ed25519 signature over `message_id || channel_id || new_ciphertext`.
    pub signature: Vec<u8>,
    pub timestamp: DateTime<Utc>,
}

/// Payload for a message deletion (tombstone).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteMessagePayload {
    pub message_id: MessageId,
    pub channel_id: ChannelId,
    pub author_id: UserId,
    /// Ed25519 signature over `message_id || channel_id || "delete"`.
    pub signature: Vec<u8>,
    pub timestamp: DateTime<Utc>,
}

/// Payload for a member departing a community.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberLeftPayload {
    pub user_id: UserId,
    pub community_id: CommunityId,
    pub timestamp: DateTime<Utc>,
    /// Ed25519 signature over `user_id || community_id || "leave"`.
    pub signature: Vec<u8>,
}

/// Payload for a channel key rotation event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelKeyRotationPayload {
    pub new_manifest: ChannelManifest,
    /// Ed25519 signature over the msgpack-encoded `new_manifest`.
    pub signature: Vec<u8>,
}

/// Payload broadcast when an admin changes a member's role.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberRoleUpdatedPayload {
    pub user_id: UserId,
    pub community_id: CommunityId,
    pub new_role: Role,
    pub timestamp: DateTime<Utc>,
    /// Ed25519 signature over `user_id || community_id || postcard(new_role) || "role_update"`.
    /// Signed by the acting admin's Ed25519 key.
    pub signature: Vec<u8>,
}

/// Payload broadcast when a new channel is created, carrying the channel manifest.
/// Each member unwraps their own channel key from `manifest.encrypted_channel_key`
/// using their X25519 secret key (ECIES). Relay nodes store the manifest for forwarding
/// without ever having access to the plaintext key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelManifestBroadcastPayload {
    pub manifest: ChannelManifest,
}

/// Presence status broadcast by a node every 30 seconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresenceHeartbeatPayload {
    pub user_id: UserId,
    pub status: String,
    pub timestamp: DateTime<Utc>,
    /// Ed25519 signature over `user_id || status || timestamp_ms_le`.
    pub signature: Vec<u8>,
}

/// Wire-format enum for all network events in BitCord.
///
/// Encoded with `postcard` for compact binary transport.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NetworkEvent {
    NewMessage(RawMessage),
    EditMessage(EditMessagePayload),
    DeleteMessage(DeleteMessagePayload),
    MemberJoined(MembershipRecord),
    MemberLeft(MemberLeftPayload),
    ChannelKeyRotation(ChannelKeyRotationPayload),
    ManifestUpdate(SignedManifest),
    PresenceHeartbeat(PresenceHeartbeatPayload),
    ChannelManifestBroadcast(ChannelManifestBroadcastPayload),
    MemberRoleUpdated(MemberRoleUpdatedPayload),
}

impl NetworkEvent {
    /// Encode to postcard bytes for transport.
    pub fn encode(&self) -> Result<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| anyhow::anyhow!(e))
    }

    /// Decode from postcard bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        membership::Role,
        types::{ChannelId, CommunityId, MessageId, UserId},
    };
    use chrono::Utc;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn make_raw_message() -> RawMessage {
        use crate::model::message::RawMessage;
        let key = SigningKey::generate(&mut OsRng);
        RawMessage::create(
            ChannelId::new(),
            &key,
            Utc::now(),
            b"ciphertext".to_vec(),
            [0u8; 24],
        )
    }

    #[test]
    fn new_message_roundtrip() {
        let event = NetworkEvent::NewMessage(make_raw_message());
        let bytes = event.encode().unwrap();
        let restored = NetworkEvent::decode(&bytes).unwrap();
        assert!(matches!(restored, NetworkEvent::NewMessage(_)));
    }

    #[test]
    fn edit_message_roundtrip() {
        let event = NetworkEvent::EditMessage(EditMessagePayload {
            message_id: MessageId::new(),
            channel_id: ChannelId::new(),
            author_id: UserId([0u8; 32]),
            new_ciphertext: b"new-ct".to_vec(),
            new_nonce: [1u8; 24],
            signature: vec![0u8; 64],
            timestamp: Utc::now(),
        });
        let bytes = event.encode().unwrap();
        let restored = NetworkEvent::decode(&bytes).unwrap();
        assert!(matches!(restored, NetworkEvent::EditMessage(_)));
    }

    #[test]
    fn delete_message_roundtrip() {
        let event = NetworkEvent::DeleteMessage(DeleteMessagePayload {
            message_id: MessageId::new(),
            channel_id: ChannelId::new(),
            author_id: UserId([1u8; 32]),
            signature: vec![0u8; 64],
            timestamp: Utc::now(),
        });
        let bytes = event.encode().unwrap();
        assert!(matches!(
            NetworkEvent::decode(&bytes).unwrap(),
            NetworkEvent::DeleteMessage(_)
        ));
    }

    #[test]
    fn member_joined_roundtrip() {
        let record = MembershipRecord {
            user_id: UserId([2u8; 32]),
            community_id: CommunityId::new(),
            display_name: "Alice".into(),
            avatar_cid: None,
            joined_at: Utc::now(),
            roles: vec![Role::Member],
            public_key: [3u8; 32],
            x25519_public_key: [4u8; 32],
            signature: vec![0u8; 64],
        };
        let event = NetworkEvent::MemberJoined(record);
        let bytes = event.encode().unwrap();
        assert!(matches!(
            NetworkEvent::decode(&bytes).unwrap(),
            NetworkEvent::MemberJoined(_)
        ));
    }

    #[test]
    fn member_left_roundtrip() {
        let event = NetworkEvent::MemberLeft(MemberLeftPayload {
            user_id: UserId([5u8; 32]),
            community_id: CommunityId::new(),
            timestamp: Utc::now(),
            signature: vec![0u8; 64],
        });
        let bytes = event.encode().unwrap();
        assert!(matches!(
            NetworkEvent::decode(&bytes).unwrap(),
            NetworkEvent::MemberLeft(_)
        ));
    }

    #[test]
    fn presence_heartbeat_roundtrip() {
        let event = NetworkEvent::PresenceHeartbeat(PresenceHeartbeatPayload {
            user_id: UserId([6u8; 32]),
            status: "online".into(),
            timestamp: Utc::now(),
            signature: vec![0u8; 64],
        });
        let bytes = event.encode().unwrap();
        assert!(matches!(
            NetworkEvent::decode(&bytes).unwrap(),
            NetworkEvent::PresenceHeartbeat(_)
        ));
    }

    #[test]
    fn manifest_update_roundtrip() {
        use crate::model::community::CommunityManifest;
        use crate::model::types::UserId;
        let admin_key = SigningKey::generate(&mut OsRng);
        let vk = admin_key.verifying_key();
        let admin_id = UserId::from_verifying_key(&vk);
        let manifest = CommunityManifest {
            id: CommunityId::new(),
            name: "Community".into(),
            description: "Desc".into(),
            public_key: vk.to_bytes(),
            created_at: Utc::now(),
            admin_ids: vec![admin_id],
            channel_ids: vec![],
            seed_nodes: vec![],
            version: 1,
            deleted: false,
        };
        let signed = manifest.sign(&admin_key);
        let event = NetworkEvent::ManifestUpdate(signed);
        let bytes = event.encode().unwrap();
        let restored = NetworkEvent::decode(&bytes).unwrap();
        if let NetworkEvent::ManifestUpdate(sm) = restored {
            assert!(sm.verify());
        } else {
            panic!("wrong variant");
        }
    }
}
