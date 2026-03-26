mod channel;
mod community;
mod dm;
mod identity;
mod message;
mod node;
mod subscription;

use std::sync::Arc;

use jsonrpsee::{RpcModule, types::ErrorObjectOwned};

use super::types::ChannelKindDto;
use super::{AppState, types::ChannelInfo};
use crate::model::channel::{ChannelKind, ChannelManifest};

// ── Error helpers ─────────────────────────────────────────────────────────────

fn internal_err(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32603, msg.into(), None::<()>)
}

fn not_found(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32001, msg.into(), None::<()>)
}

fn invalid_params(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32602, msg.into(), None::<()>)
}

fn forbidden(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32003, msg.into(), None::<()>)
}

/// Decode a 64-character lowercase hex string into a 32-byte SHA-256 fingerprint.
/// Returns `None` if the string is not exactly 64 valid hex characters.
fn parse_fingerprint_hex(h: &str) -> Option<[u8; 32]> {
    if h.len() != 64 {
        return None;
    }
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
}

fn seed_unavailable() -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        -32004,
        "seed peer is not connected — this community is currently unreachable",
        None::<()>,
    )
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` if any of the community's seed nodes points back at our own
/// QUIC server (self-hosted community).  Checked against our NAT-discovered
/// public address and any known listen addresses.
async fn we_are_seed(ctx: &AppState, seed_nodes: &[String]) -> bool {
    if seed_nodes.is_empty() {
        return false;
    }
    if let Some(pub_addr) = ctx.public_addr.read().await.clone() {
        if seed_nodes.contains(&pub_addr) {
            return true;
        }
    }
    let listen = ctx.actual_listen_addrs.read().await;
    seed_nodes.iter().any(|s| listen.contains(s))
}

/// Returns `Err(seed_unavailable())` if the community has seed nodes configured,
/// we are NOT the host, and no seed peer is currently connected.
async fn require_seed_connected(
    ctx: &Arc<AppState>,
    community_id: &str,
) -> Result<(), ErrorObjectOwned> {
    let communities = ctx.communities.read().await;
    let signed = communities
        .get(community_id)
        .ok_or_else(|| not_found("community not found"))?;
    let m = &signed.manifest;
    if m.seed_nodes.is_empty() {
        return Ok(());
    }
    if we_are_seed(ctx, &m.seed_nodes).await {
        return Ok(());
    }
    let seed_connected = ctx.seed_connected_communities.read().await;
    if seed_connected.contains(community_id) {
        Ok(())
    } else {
        Err(seed_unavailable())
    }
}

fn channel_manifest_to_info(m: &ChannelManifest) -> ChannelInfo {
    ChannelInfo {
        id: m.id.to_string(),
        community_id: m.community_id.to_string(),
        name: m.name.clone(),
        kind: match m.kind {
            ChannelKind::Text => ChannelKindDto::Text,
            ChannelKind::Announcement => ChannelKindDto::Announcement,
            ChannelKind::Voice => ChannelKindDto::Voice,
        },
        version: m.version,
        created_at: m.created_at,
    }
}

// ── Module builder ────────────────────────────────────────────────────────────

/// Build the complete JSON-RPC module with all methods and the push-event subscription.
///
/// The returned `RpcModule<Arc<AppState>>` is passed to `jsonrpsee::server::Server::start`.
pub fn build_rpc_module(state: Arc<AppState>) -> anyhow::Result<RpcModule<Arc<AppState>>> {
    let mut module = RpcModule::new(state);
    identity::register_identity_methods(&mut module)?;
    community::register_community_methods(&mut module)?;
    channel::register_channel_methods(&mut module)?;
    message::register_message_methods(&mut module)?;
    dm::register_dm_methods(&mut module)?;
    node::register_node_methods(&mut module)?;
    subscription::register_subscription(&mut module)?;
    Ok(module)
}
