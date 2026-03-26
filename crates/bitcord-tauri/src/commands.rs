//! Tauri commands — QUIC node operations.
//!
//! These commands wrap `NodeClient` to give the frontend direct access to the
//! encrypted messaging layer without going through the JSON-RPC bridge.

use std::{collections::HashMap, sync::Arc};

use bitcord_core::{
    crypto::{channel_keys::ChannelKey, dm::DmEnvelope},
    identity::NodeIdentity,
    network::client::NodeClient,
};
use serde::Serialize;
use tauri::State;
use tokio::sync::RwLock;
use x25519_dalek::PublicKey as X25519PublicKey;

use bitcord_core::model::community::SignedManifest;

// ── Managed state ─────────────────────────────────────────────────────────────

/// Tauri-managed state for QUIC node operations.
///
/// Stored via `app.manage(NodeState { ... })` and injected into commands as
/// `State<'_, NodeState>`.
pub struct NodeState {
    /// This node's cryptographic identity.
    pub identity: Arc<NodeIdentity>,
    /// The local embedded node connection (set once after `init_backend`).
    pub local_client: tokio::sync::Mutex<Option<NodeClient>>,
    /// Remote node connections keyed by community public key (base58).
    pub remote_clients: tokio::sync::Mutex<HashMap<String, NodeClient>>,
    /// Per-channel symmetric keys shared with `AppState`.
    pub channel_keys: Arc<RwLock<HashMap<String, [u8; 32]>>>,
    /// Community manifests containing the 32-byte public keys.
    pub communities: Arc<RwLock<HashMap<String, SignedManifest>>>,
}

// ── Response types ────────────────────────────────────────────────────────────

/// A single decrypted channel message returned to the frontend.
#[derive(Serialize, Clone)]
pub struct DecryptedMessage {
    pub seq: u64,
    pub message_id: String,
    pub author_id: String,
    pub timestamp_ms: i64,
    pub plaintext: String,
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Encrypt `plaintext` with the channel's symmetric key and send it to the
/// embedded node.
///
/// Returns the sequence number assigned by the node.
#[tauri::command]
pub async fn node_send_message(
    state: State<'_, NodeState>,
    community_id: String,
    channel_id: String,
    plaintext: String,
) -> Result<u64, String> {
    let community_pk = {
        let comms = state.communities.read().await;
        if let Some(signed) = comms.get(&community_id) {
            signed.manifest.public_key
        } else {
            // Fallback: pad the 16-byte ULID with zeros.
            let u = community_id
                .parse::<ulid::Ulid>()
                .map_err(|e| e.to_string())?;
            let mut bytes = [0u8; 32];
            bytes[..16].copy_from_slice(&u.to_bytes());
            bytes
        }
    };
    let channel_ulid = channel_id
        .parse::<ulid::Ulid>()
        .map_err(|e| e.to_string())?;

    let key_bytes = {
        let keys = state.channel_keys.read().await;
        *keys
            .get(&channel_id)
            .ok_or("no channel key for this channel")?
    };
    let channel_key = ChannelKey::from_bytes(key_bytes);
    let (nonce, ciphertext) = channel_key
        .encrypt_message(plaintext.as_bytes())
        .map_err(|e| e.to_string())?;

    let guard = state.local_client.lock().await;
    let client = guard.as_ref().ok_or("embedded node not yet connected")?;
    client
        .send_message(community_pk, channel_ulid, nonce, ciphertext)
        .await
        .map_err(|e| e.to_string())
}

/// Fetch encrypted messages from the embedded node since `since_seq`, decrypt
/// them using the channel's symmetric key, and return the plaintext messages.
#[tauri::command]
pub async fn node_get_messages(
    state: State<'_, NodeState>,
    community_id: String,
    channel_id: String,
    since_seq: u64,
) -> Result<Vec<DecryptedMessage>, String> {
    let community_pk = {
        let comms = state.communities.read().await;
        if let Some(signed) = comms.get(&community_id) {
            signed.manifest.public_key
        } else {
            // Fallback: pad the 16-byte ULID with zeros.
            let u = community_id
                .parse::<ulid::Ulid>()
                .map_err(|e| e.to_string())?;
            let mut bytes = [0u8; 32];
            bytes[..16].copy_from_slice(&u.to_bytes());
            bytes
        }
    };
    let channel_ulid = channel_id
        .parse::<ulid::Ulid>()
        .map_err(|e| e.to_string())?;

    let entries = {
        let guard = state.local_client.lock().await;
        let client = guard.as_ref().ok_or("embedded node not yet connected")?;
        client
            .get_messages(community_pk, channel_ulid, since_seq)
            .await
            .map_err(|e| e.to_string())?
    };

    let key_bytes = {
        let keys = state.channel_keys.read().await;
        *keys
            .get(&channel_id)
            .ok_or("no channel key for this channel")?
    };
    let channel_key = ChannelKey::from_bytes(key_bytes);

    let mut result = Vec::with_capacity(entries.len());
    for entry in entries {
        let plaintext_bytes = channel_key
            .decrypt_message(&entry.nonce, &entry.ciphertext)
            .map_err(|e| format!("decrypt seq {}: {e}", entry.seq))?;
        let plaintext = String::from_utf8(plaintext_bytes).unwrap_or_else(|_| "<binary>".into());
        result.push(DecryptedMessage {
            seq: entry.seq,
            message_id: entry.message_id,
            author_id: entry.author_id,
            timestamp_ms: entry.timestamp_ms,
            plaintext,
        });
    }
    Ok(result)
}

/// Seal `plaintext` as a DM for `recipient_pk_b58` (base58 X25519 public key)
/// and deliver it via the embedded node.
///
/// Returns the sequence number in the recipient's mailbox.
#[tauri::command]
pub async fn node_send_dm(
    state: State<'_, NodeState>,
    recipient_pk_b58: String,
    plaintext: String,
) -> Result<u64, String> {
    let recipient_bytes: [u8; 32] = bs58::decode(&recipient_pk_b58)
        .into_vec()
        .map_err(|e| e.to_string())?
        .try_into()
        .map_err(|_| "recipient public key must be exactly 32 bytes".to_string())?;
    let recipient_pk = X25519PublicKey::from(recipient_bytes);

    let sender_sk = state.identity.x25519_secret();
    let envelope = DmEnvelope::seal(&sender_sk, &recipient_pk, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;

    let guard = state.local_client.lock().await;
    let client = guard.as_ref().ok_or("embedded node not yet connected")?;
    client
        .send_dm(recipient_bytes, envelope)
        .await
        .map_err(|e| e.to_string())
}
