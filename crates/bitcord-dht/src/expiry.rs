//! Background TTL expiry tasks for DHT records.

use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use crate::{routing::DhtState, store::DhtStore};

/// Spawn background tasks that expire stale DHT records.
///
/// - Every 10 minutes: expire in-memory records and prune the on-disk store.
/// - Every 1 hour: persist current in-memory community peer snapshot to disk.
pub fn spawn_expiry_task(state: Arc<DhtState>, store: Arc<DhtStore>) {
    {
        let state_e = Arc::clone(&state);
        let store_e = Arc::clone(&store);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(600));
            loop {
                interval.tick().await;
                state_e.expire_records();
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let peer_cutoff = now_secs.saturating_sub(crate::routing::COMMUNITY_PEER_TTL_SECS);
                if let Err(e) = store_e.remove_expired_community_peers(peer_cutoff) {
                    warn!("failed to prune expired DHT community peers: {e}");
                }
                let info_cutoff = now_secs.saturating_sub(crate::routing::PEER_INFO_TTL_SECS);
                if let Err(e) = store_e.remove_expired_peer_infos(info_cutoff) {
                    warn!("failed to prune expired DHT peer info records: {e}");
                }
            }
        });
    }
    {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;
                let snapshot = state.all_community_peers();
                for (community_pk, records) in &snapshot {
                    for record in records {
                        if let Err(e) = store.set_community_peer_record(community_pk, record) {
                            warn!("failed to persist DHT community peer record: {e}");
                        }
                    }
                }
            }
        });
    }
}
