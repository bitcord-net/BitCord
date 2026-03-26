use std::collections::HashMap;

/// Tracks the last-seen sequence number per channel.
///
/// Used to compute unread counts: any `LogEntry` with `seq > last_read(channel_id)`
/// is considered unread.
#[derive(Debug, Default)]
pub struct ReadState {
    map: HashMap<String, u64>,
}

impl ReadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that the user has read up to `seq` in `channel_id`.
    pub fn mark_read(&mut self, channel_id: &str, seq: u64) {
        self.map.insert(channel_id.to_string(), seq);
    }

    /// Return the last-read sequence number for `channel_id`, or 0 if never read.
    pub fn last_read(&self, channel_id: &str) -> u64 {
        self.map.get(channel_id).copied().unwrap_or(0)
    }

    /// Unread count = total entries - (last_read + 1), or total if never read.
    pub fn unread_count(&self, channel_id: &str, total: u64) -> u64 {
        if total == 0 {
            return 0;
        }
        let last = self.map.get(channel_id).copied();
        match last {
            Some(seq) => total.saturating_sub(seq + 1),
            None => total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_and_retrieve() {
        let mut rs = ReadState::new();
        rs.mark_read("chan1", 5);
        assert_eq!(rs.last_read("chan1"), 5);
    }

    #[test]
    fn unread_count() {
        let mut rs = ReadState::new();
        rs.mark_read("chan1", 4);
        // 10 total, read up to seq 4 (5 entries 0..=4) → 5 unread (5..=9)
        assert_eq!(rs.unread_count("chan1", 10), 5);
    }

    #[test]
    fn never_read_all_unread() {
        let rs = ReadState::new();
        assert_eq!(rs.unread_count("chan1", 7), 7);
    }
}
