use crate::model::types::{ChannelId, MessageId, UserId};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A reference to a file attachment stored by content address.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// Content identifier (e.g. IPFS CID or SHA-256 hex).
    pub cid: String,
    pub name: String,
    pub size: u64,
    pub mime_type: String,
}

/// Legacy struct format for `MessageContent` — used only when deserializing entries written
/// before the enum migration. New code always writes the enum format.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LegacyMessageContent {
    pub body: String,
    pub attachments: Vec<AttachmentRef>,
    pub reply_to: Option<MessageId>,
    pub edited_at: Option<DateTime<Utc>>,
}

/// The decrypted inner content of a channel log entry.
///
/// `Text` entries are user messages; `Reaction` entries represent an emoji
/// reaction add or remove and act as the source-of-truth for reaction state
/// so that history sync automatically delivers reactions to offline peers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageContent {
    Text {
        body: String,
        attachments: Vec<AttachmentRef>,
        reply_to: Option<MessageId>,
        edited_at: Option<DateTime<Utc>>,
    },
    Reaction {
        /// The `message_id` of the text entry being reacted to.
        target_message_id: String,
        emoji: String,
        is_add: bool,
    },
}

impl MessageContent {
    /// Decode from postcard bytes.
    ///
    /// Tries the current enum format first, then falls back to the legacy struct
    /// format for entries written before the enum migration.
    pub fn decode(plaintext: &[u8]) -> Option<Self> {
        postcard::from_bytes::<MessageContent>(plaintext)
            .ok()
            .or_else(|| {
                postcard::from_bytes::<LegacyMessageContent>(plaintext)
                    .ok()
                    .map(|l| MessageContent::Text {
                        body: l.body,
                        attachments: l.attachments,
                        reply_to: l.reply_to,
                        edited_at: l.edited_at,
                    })
            })
    }
}

/// The on-wire encrypted message struct distributed over GossipSub.
///
/// `ciphertext` is `MessageContent` serialized to msgpack then encrypted with
/// the channel's `ChannelKey`. The 24-byte `nonce` is stored alongside it.
///
/// The author's signature covers: `id || channel_id || timestamp_ms(le) || ciphertext`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawMessage {
    pub id: MessageId,
    pub channel_id: ChannelId,
    pub author_id: UserId,
    pub timestamp: DateTime<Utc>,
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 24],
    /// Ed25519 signature (64 bytes).
    pub signature: Vec<u8>,
}

impl RawMessage {
    /// Build the canonical byte string that the author must sign.
    ///
    /// Format: `id_bytes(16) || channel_id_bytes(16) || timestamp_ms_le(8) || ciphertext`.
    pub fn signable_bytes(
        id: &MessageId,
        channel_id: &ChannelId,
        timestamp: &DateTime<Utc>,
        ciphertext: &[u8],
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16 + 16 + 8 + ciphertext.len());
        buf.extend_from_slice(&id.0.to_bytes());
        buf.extend_from_slice(&channel_id.0.to_bytes());
        buf.extend_from_slice(&timestamp.timestamp_millis().to_le_bytes());
        buf.extend_from_slice(ciphertext);
        buf
    }

    /// Create and sign a new `RawMessage`.
    pub fn create(
        channel_id: ChannelId,
        author_key: &SigningKey,
        timestamp: DateTime<Utc>,
        ciphertext: Vec<u8>,
        nonce: [u8; 24],
    ) -> Self {
        let id = MessageId(Ulid::new());
        let author_id = UserId::from_verifying_key(&author_key.verifying_key());
        let signable = Self::signable_bytes(&id, &channel_id, &timestamp, &ciphertext);
        let signature = author_key.sign(&signable).to_bytes().to_vec();
        RawMessage {
            id,
            channel_id,
            author_id,
            timestamp,
            ciphertext,
            nonce,
            signature,
        }
    }

    /// Verify that `signature` is a valid Ed25519 signature from `author_pubkey`.
    pub fn verify(&self, author_pubkey: &VerifyingKey) -> bool {
        let Ok(sig_bytes): Result<[u8; 64], _> = self.signature.as_slice().try_into() else {
            return false;
        };
        let sig = Signature::from_bytes(&sig_bytes);
        let signable = Self::signable_bytes(
            &self.id,
            &self.channel_id,
            &self.timestamp,
            &self.ciphertext,
        );
        author_pubkey.verify(&signable, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn create_and_verify() {
        let key = SigningKey::generate(&mut OsRng);
        let channel_id = ChannelId::new();
        let msg = RawMessage::create(
            channel_id,
            &key,
            Utc::now(),
            b"encrypted-payload".to_vec(),
            [0u8; 24],
        );
        assert!(msg.verify(&key.verifying_key()));
    }

    #[test]
    fn tampered_ciphertext_fails_verification() {
        let key = SigningKey::generate(&mut OsRng);
        let mut msg = RawMessage::create(
            ChannelId::new(),
            &key,
            Utc::now(),
            b"original".to_vec(),
            [0u8; 24],
        );
        msg.ciphertext = b"tampered".to_vec();
        assert!(!msg.verify(&key.verifying_key()));
    }

    #[test]
    fn serde_roundtrip_json() {
        let key = SigningKey::generate(&mut OsRng);
        let msg = RawMessage::create(
            ChannelId::new(),
            &key,
            Utc::now(),
            b"payload".to_vec(),
            [1u8; 24],
        );
        let json = serde_json::to_string(&msg).unwrap();
        let restored: RawMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg.id, restored.id);
        assert!(restored.verify(&key.verifying_key()));
    }

    #[test]
    fn serde_roundtrip_postcard() {
        let key = SigningKey::generate(&mut OsRng);
        let msg = RawMessage::create(
            ChannelId::new(),
            &key,
            Utc::now(),
            b"payload".to_vec(),
            [2u8; 24],
        );
        let bytes = postcard::to_allocvec(&msg).unwrap();
        let restored: RawMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(msg.id, restored.id);
        assert!(restored.verify(&key.verifying_key()));
    }

    #[test]
    fn message_content_text_serde() {
        let content = MessageContent::Text {
            body: "Hello, world!".into(),
            attachments: vec![AttachmentRef {
                cid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi".into(),
                name: "file.txt".into(),
                size: 1024,
                mime_type: "text/plain".into(),
            }],
            reply_to: Some(MessageId::new()),
            edited_at: None,
        };
        let bytes = postcard::to_allocvec(&content).unwrap();
        let restored = MessageContent::decode(&bytes).unwrap();
        assert!(
            matches!(restored, MessageContent::Text { ref body, .. } if body == "Hello, world!")
        );
    }

    #[test]
    fn message_content_reaction_serde() {
        let content = MessageContent::Reaction {
            target_message_id: "01ABCDEF".into(),
            emoji: "👍".into(),
            is_add: true,
        };
        let bytes = postcard::to_allocvec(&content).unwrap();
        let restored = MessageContent::decode(&bytes).unwrap();
        assert!(matches!(
            restored,
            MessageContent::Reaction { is_add: true, .. }
        ));
    }

    #[test]
    fn legacy_message_content_decoded_as_text() {
        // Simulate an entry written in the old struct format.
        let legacy = LegacyMessageContent {
            body: "legacy message".into(),
            attachments: vec![],
            reply_to: None,
            edited_at: None,
        };
        let bytes = postcard::to_allocvec(&legacy).unwrap();
        let restored = MessageContent::decode(&bytes).unwrap();
        assert!(
            matches!(restored, MessageContent::Text { ref body, .. } if body == "legacy message")
        );
    }
}
