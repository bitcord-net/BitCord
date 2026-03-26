use std::collections::HashMap;

/// A single entry in the append-only encrypted message log.
///
/// `seq` is a monotonically increasing sequence number per channel, starting at 0.
/// The `nonce` and `ciphertext` form the encrypted `MessageContent` blob.
///
/// The `message_id`, `author_id`, and `timestamp_ms` fields are stored
/// alongside the ciphertext for efficient querying without decryption.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
    pub message_id: String,
    pub author_id: String,
    pub timestamp_ms: i64,
    #[serde(default)]
    pub deleted: bool,
}

/// In-memory per-channel append-only message log.
///
/// Channels are identified by their ULID string. Entries are stored in
/// insertion order with a monotonically increasing `seq` (= index in the Vec).
///
/// For persistent storage across restarts, use the `redb`-backed node store.
#[derive(Debug, Default)]
pub struct MessageLog {
    channels: HashMap<String, Vec<LogEntry>>,
    /// Per-message reactions: message_id → emoji → list of user_ids.
    reactions: HashMap<String, HashMap<String, Vec<String>>>,
}

impl MessageLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a new entry to a channel log and return the assigned sequence number.
    pub fn append(
        &mut self,
        channel_id: &str,
        message_id: String,
        author_id: String,
        timestamp_ms: i64,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
    ) -> u64 {
        let entries = self.channels.entry(channel_id.to_string()).or_default();
        let seq = entries.len() as u64;
        entries.push(LogEntry {
            seq,
            nonce,
            ciphertext,
            message_id,
            author_id,
            timestamp_ms,
            deleted: false,
        });
        seq
    }

    /// Append a pre-constructed LogEntry to a channel log.
    ///
    /// The entry's `seq` number is preserved. If the channel does not exist,
    /// it is created.
    pub fn append_entry(&mut self, channel_id: &str, entry: LogEntry) {
        self.channels
            .entry(channel_id.to_string())
            .or_default()
            .push(entry);
    }

    /// Return all entries for a channel with `seq >= since_seq`.
    pub fn get_since(&self, channel_id: &str, since_seq: u64) -> &[LogEntry] {
        match self.channels.get(channel_id) {
            Some(entries) => {
                let start = since_seq as usize;
                if start >= entries.len() {
                    &[]
                } else {
                    &entries[start..]
                }
            }
            None => &[],
        }
    }

    /// Return a reference to a single entry by message_id, if it exists.
    pub fn get_entry(&self, channel_id: &str, message_id: &str) -> Option<&LogEntry> {
        self.channels
            .get(channel_id)?
            .iter()
            .find(|e| e.message_id == message_id)
    }

    /// Update the ciphertext of an existing entry (edit operation).
    ///
    /// Returns `true` if the entry was found and updated, `false` if not found.
    pub fn edit(
        &mut self,
        channel_id: &str,
        message_id: &str,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
    ) -> bool {
        match self.channels.get_mut(channel_id) {
            Some(entries) => {
                if let Some(entry) = entries.iter_mut().find(|e| e.message_id == message_id) {
                    entry.nonce = nonce;
                    entry.ciphertext = ciphertext;
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Mark an entry as deleted (tombstone operation).
    ///
    /// Returns `true` if the entry was found and tombstoned, `false` if not found.
    pub fn tombstone(&mut self, channel_id: &str, message_id: &str) -> bool {
        match self.channels.get_mut(channel_id) {
            Some(entries) => {
                if let Some(entry) = entries.iter_mut().find(|e| e.message_id == message_id) {
                    entry.deleted = true;
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Total number of entries in a channel log.
    pub fn len(&self, channel_id: &str) -> u64 {
        self.channels
            .get(channel_id)
            .map(|e| e.len() as u64)
            .unwrap_or(0)
    }

    /// Add a reaction from `user_id` to `message_id` for `emoji`.
    ///
    /// Returns `true` if the reaction was newly added, `false` if it already existed.
    pub fn react(&mut self, message_id: &str, emoji: &str, user_id: &str) -> bool {
        let users = self
            .reactions
            .entry(message_id.to_string())
            .or_default()
            .entry(emoji.to_string())
            .or_default();
        if users.contains(&user_id.to_string()) {
            return false;
        }
        users.push(user_id.to_string());
        true
    }

    /// Remove a reaction from `user_id` to `message_id` for `emoji`.
    ///
    /// Returns `true` if the reaction was found and removed, `false` if it wasn't present.
    pub fn unreact(&mut self, message_id: &str, emoji: &str, user_id: &str) -> bool {
        if let Some(by_emoji) = self.reactions.get_mut(message_id) {
            if let Some(users) = by_emoji.get_mut(emoji) {
                let before = users.len();
                users.retain(|u| u != user_id);
                let removed = users.len() < before;
                if users.is_empty() {
                    by_emoji.remove(emoji);
                }
                return removed;
            }
        }
        false
    }

    /// Return all reactions for a message as a vec of `(emoji, user_ids)` pairs.
    pub fn get_reactions(&self, message_id: &str) -> Vec<(String, Vec<String>)> {
        self.reactions
            .get(message_id)
            .map(|by_emoji| {
                let mut v: Vec<_> = by_emoji
                    .iter()
                    .map(|(e, ids)| (e.clone(), ids.clone()))
                    .collect();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_retrieve() {
        let mut log = MessageLog::new();
        let seq = log.append(
            "chan1",
            "id1".into(),
            "author1".into(),
            1000,
            [0u8; 24],
            vec![1, 2, 3],
        );
        assert_eq!(seq, 0);
        let seq2 = log.append(
            "chan1",
            "id2".into(),
            "author1".into(),
            2000,
            [1u8; 24],
            vec![4, 5, 6],
        );
        assert_eq!(seq2, 1);
        let entries = log.get_since("chan1", 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[1].seq, 1);
    }

    #[test]
    fn get_since_filters_correctly() {
        let mut log = MessageLog::new();
        for i in 0u64..5 {
            log.append(
                "chan",
                format!("id{i}"),
                "a".into(),
                i as i64 * 1000,
                [0u8; 24],
                vec![],
            );
        }
        let entries = log.get_since("chan", 3);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 3);
    }

    #[test]
    fn get_since_empty_channel() {
        let log = MessageLog::new();
        assert!(log.get_since("nonexistent", 0).is_empty());
    }

    #[test]
    fn separate_channels_are_independent() {
        let mut log = MessageLog::new();
        log.append("chan_a", "id1".into(), "a".into(), 0, [0u8; 24], vec![]);
        log.append("chan_b", "id2".into(), "b".into(), 0, [0u8; 24], vec![]);
        assert_eq!(log.len("chan_a"), 1);
        assert_eq!(log.len("chan_b"), 1);
        // chan_a seq starts at 0 independently of chan_b
        assert_eq!(log.get_since("chan_a", 0)[0].seq, 0);
        assert_eq!(log.get_since("chan_b", 0)[0].seq, 0);
    }
}
