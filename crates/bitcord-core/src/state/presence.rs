use chrono::{DateTime, Utc};
use std::collections::HashMap;

const OFFLINE_THRESHOLD_SECS: i64 = 300; // 5 minutes

/// Presence status of a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceStatus {
    Online,
    Idle,
    DoNotDisturb,
    Offline,
}

/// In-memory presence map: `user_id_str → (status, last_seen)`.
///
/// Entries older than 5 minutes are treated as `Offline` on read.
#[derive(Debug, Default)]
pub struct PresenceState {
    map: HashMap<String, (PresenceStatus, DateTime<Utc>)>,
}

impl PresenceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upsert presence for a user.
    pub fn update(&mut self, user_id: &str, status: PresenceStatus, last_seen: DateTime<Utc>) {
        self.map.insert(user_id.to_string(), (status, last_seen));
    }

    /// Get current presence for a user. Returns `Offline` if stale or unknown.
    pub fn get(&self, user_id: &str) -> PresenceStatus {
        match self.map.get(user_id) {
            Some((status, last_seen)) => {
                let age = Utc::now().signed_duration_since(*last_seen).num_seconds();
                if age > OFFLINE_THRESHOLD_SECS {
                    PresenceStatus::Offline
                } else {
                    status.clone()
                }
            }
            None => PresenceStatus::Offline,
        }
    }

    /// All non-stale entries as `(user_id, status)` pairs.
    pub fn all_active(&self) -> Vec<(String, PresenceStatus)> {
        let now = Utc::now();
        self.map
            .iter()
            .filter(|(_, (_, ts))| {
                now.signed_duration_since(*ts).num_seconds() <= OFFLINE_THRESHOLD_SECS
            })
            .map(|(id, (s, _))| (id.clone(), s.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_and_get() {
        let mut ps = PresenceState::new();
        ps.update("user1", PresenceStatus::Online, Utc::now());
        assert_eq!(ps.get("user1"), PresenceStatus::Online);
    }

    #[test]
    fn stale_entry_is_offline() {
        let mut ps = PresenceState::new();
        let old = Utc::now() - chrono::Duration::minutes(10);
        ps.update("user1", PresenceStatus::Online, old);
        assert_eq!(ps.get("user1"), PresenceStatus::Offline);
    }

    #[test]
    fn unknown_user_is_offline() {
        let ps = PresenceState::new();
        assert_eq!(ps.get("nobody"), PresenceStatus::Offline);
    }

    #[test]
    fn all_active_filters_stale() {
        let mut ps = PresenceState::new();
        ps.update("u1", PresenceStatus::Online, Utc::now());
        ps.update(
            "u2",
            PresenceStatus::Idle,
            Utc::now() - chrono::Duration::minutes(10),
        );
        let active = ps.all_active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0, "u1");
    }
}
