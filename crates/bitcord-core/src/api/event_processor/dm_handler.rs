use tracing::{debug, warn};

use super::super::push_broadcaster::{DmEventData, PushEvent};
use super::super::types::DmMessageInfo;
use super::super::{AppState, save_table};

/// Decrypt an incoming DM LogEntry, persist it, and push it to the frontend.
pub(super) async fn handle_dm_received(
    state: &AppState,
    entry: crate::state::message_log::LogEntry,
    recipient_pk: [u8; 32],
) {
    // Skip decryption if this DM is not addressed to us.
    let local_x25519_pk =
        crate::identity::NodeIdentity::from_signing_key_bytes(&state.signing_key.to_bytes())
            .x25519_public_key_bytes();
    if recipient_pk != local_x25519_pk {
        return;
    }

    // Deserialize the ciphertext as a DmEnvelope.
    let envelope: crate::crypto::dm::DmEnvelope = match postcard::from_bytes(&entry.ciphertext) {
        Ok(e) => e,
        Err(e) => {
            warn!(author_id = %entry.author_id, "dm: failed to deserialize DmEnvelope: {e}");
            return;
        }
    };

    // Derive local user's X25519 secret key from their Ed25519 signing key.
    let local_x25519_sk =
        crate::identity::NodeIdentity::from_signing_key_bytes(&state.signing_key.to_bytes())
            .x25519_secret();

    // Decrypt the envelope.
    let plaintext = match envelope.open(&local_x25519_sk) {
        Ok(p) => p,
        Err(e) => {
            warn!(author_id = %entry.author_id, "dm: decryption failed: {e}");
            return;
        }
    };

    // Try to decode as a structured DmPayload (new format).
    // Fall back to raw UTF-8 for envelopes from older clients.
    let (body, reply_to, payload_id) = match postcard::from_bytes::<crate::crypto::dm::DmPayload>(
        &plaintext,
    ) {
        Ok(p) => {
            debug!(
                author_id = %entry.author_id,
                body_len = p.body.len(),
                reply_to = ?p.reply_to,
                payload_id = %p.id,
                "dm: decoded DmPayload"
            );
            let id = if p.id.is_empty() { None } else { Some(p.id) };
            (p.body, p.reply_to, id)
        }
        Err(e) => {
            debug!(author_id = %entry.author_id, "dm: DmPayload decode failed ({e}), falling back to UTF-8");
            match String::from_utf8(plaintext) {
                Ok(s) => (s, None, None),
                Err(e) => {
                    warn!(author_id = %entry.author_id, "dm: body is not valid UTF-8: {e}");
                    return;
                }
            }
        }
    };

    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(entry.timestamp_ms)
        .unwrap_or_else(chrono::Utc::now);

    // Use the sender-assigned ID if present so both parties share the same
    // canonical message ID (required for reply-quote lookups).
    let message_id = payload_id.unwrap_or(entry.message_id);

    let msg = DmMessageInfo {
        id: message_id,
        peer_id: entry.author_id.clone(),
        author_id: entry.author_id.clone(),
        timestamp,
        body,
        reply_to,
        edited_at: None,
    };
    {
        let mut dms = state.dms.write().await;
        let conversation = dms.entry(entry.author_id).or_default();
        // Deduplicate: skip if we already have a message with this ID (e.g. it
        // was delivered live before we went offline and is now re-fetched from
        // the mailbox on reconnect).
        if conversation.iter().any(|m| m.id == msg.id) {
            return;
        }
        conversation.push(msg.clone());
        save_table(
            &state.data_dir.join("dms.json"),
            &*dms,
            state.encryption_key.as_ref(),
        );
    }
    state
        .broadcaster
        .send(PushEvent::DmNew(DmEventData { message: msg }));
}
