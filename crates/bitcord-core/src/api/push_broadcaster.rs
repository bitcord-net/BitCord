use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::types::DmMessageInfo;
use crate::resource::metrics::MetricsSnapshot;

// ── Push event payload types ──────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageEventData {
    pub message_id: String,
    pub channel_id: String,
    pub community_id: String,
    pub author_id: String,
    pub author_name: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageDeletedData {
    pub message_id: String,
    pub channel_id: String,
    pub community_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberEventData {
    pub user_id: String,
    pub community_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresenceChangedData {
    pub user_id: String,
    pub status: String,
    pub last_seen: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelEventData {
    pub channel_id: String,
    pub community_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommunityEventData {
    pub community_id: String,
    pub version: u64,
    /// Human-readable reason when the deletion was caused by a join failure
    /// (e.g. wrong hosting password). Empty for normal admin-initiated deletions.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncProgressData {
    pub channel_id: String,
    /// Fraction in [0.0, 1.0].
    pub progress: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelHistorySyncedData {
    pub channel_id: String,
    pub community_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionInfo {
    pub emoji: String,
    pub user_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionUpdatedData {
    pub message_id: String,
    pub channel_id: String,
    pub community_id: String,
    pub reactions: Vec<ReactionInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DmEventData {
    pub message: DmMessageInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DmSendFailedData {
    pub peer_id: String,
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeedStatusData {
    pub community_id: String,
    pub connected: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberRoleUpdatedData {
    pub user_id: String,
    pub community_id: String,
    pub new_role: String,
}

// ── PushEvent ─────────────────────────────────────────────────────────────────

/// All server-to-client push events sent over the WebSocket connection.
///
/// Serialized as `{ "type": "...", "data": { ... } }`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PushEvent {
    MessageNew(MessageEventData),
    MessageEdited(MessageEventData),
    MessageDeleted(MessageDeletedData),
    MemberJoined(MemberEventData),
    MemberLeft(MemberEventData),
    PresenceChanged(PresenceChangedData),
    ChannelCreated(ChannelEventData),
    ChannelDeleted(ChannelEventData),
    CommunityManifestUpdated(CommunityEventData),
    CommunityDeleted(CommunityEventData),
    NodeMetricsUpdated(MetricsSnapshot),
    SyncProgress(SyncProgressData),
    DmNew(DmEventData),
    DmSendFailed(DmSendFailedData),
    ChannelHistorySynced(ChannelHistorySyncedData),
    ReactionUpdated(ReactionUpdatedData),
    SeedStatusChanged(SeedStatusData),
    MemberRoleUpdated(MemberRoleUpdatedData),
}

// ── PushBroadcaster ───────────────────────────────────────────────────────────

/// Holds the sending half of a broadcast channel for push events.
///
/// The swarm event loop (and metrics task) call [`PushBroadcaster::send`] to
/// fanout events to all subscribed WebSocket connections.
pub struct PushBroadcaster {
    pub sender: broadcast::Sender<PushEvent>,
}

impl Default for PushBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl PushBroadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender }
    }

    /// Subscribe to receive a copy of every future push event.
    pub fn subscribe(&self) -> broadcast::Receiver<PushEvent> {
        self.sender.subscribe()
    }

    /// Broadcast an event to all current subscribers (fire-and-forget).
    pub fn send(&self, event: PushEvent) {
        let _ = self.sender.send(event);
    }
}

// Make MetricsSnapshot serializable for push events.
impl Serialize for MetricsSnapshot {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("MetricsSnapshot", 6)?;
        st.serialize_field("connected_peers", &self.connected_peers)?;
        st.serialize_field("stored_channels", &self.stored_channels)?;
        st.serialize_field("disk_usage_mb", &self.disk_usage_mb)?;
        st.serialize_field("bandwidth_in_kbps", &self.bandwidth_in_kbps)?;
        st.serialize_field("bandwidth_out_kbps", &self.bandwidth_out_kbps)?;
        st.serialize_field("uptime_secs", &self.uptime_secs)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for MetricsSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Helper {
            connected_peers: u64,
            stored_channels: u64,
            disk_usage_mb: u64,
            bandwidth_in_kbps: u64,
            bandwidth_out_kbps: u64,
            uptime_secs: u64,
        }
        let h = Helper::deserialize(d)?;
        Ok(MetricsSnapshot {
            connected_peers: h.connected_peers,
            stored_channels: h.stored_channels,
            disk_usage_mb: h.disk_usage_mb,
            bandwidth_in_kbps: h.bandwidth_in_kbps,
            bandwidth_out_kbps: h.bandwidth_out_kbps,
            uptime_secs: h.uptime_secs,
        })
    }
}
