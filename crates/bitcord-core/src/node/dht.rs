//! Lightweight DHT for mailbox routing.
//!
//! # Design
//! Maintains a **k-bucket routing table** (K=20, Kademlia-style XOR metric) so
//! that the node can route `FIND_NODE` queries across the network, combined with
//! a mailbox record store with TTL-based expiry.
//!
//! # Record types
//! - `mailbox/<user_pk>` → `(NodeAddr, expires_at)` — which node holds a user's mailbox
//!
//! # Distributed operation
//! `FindNode` / `StoreDhtRecord` RPCs are defined in `protocol.rs` and handled
//! in `handler.rs`.  The iterative lookup in `network_handle.rs` drives multi-hop
//! resolution so that `lookup_mailbox` can resolve addresses across the network.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::network::NodeAddr;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of peers per k-bucket (standard Kademlia K value).
const K: usize = 20;

/// Mailbox record TTL: 24 hours.
const MAILBOX_TTL: Duration = Duration::from_secs(86_400);

// ── NodeId ────────────────────────────────────────────────────────────────────

/// 256-bit DHT node identifier derived from the node's Ed25519 public key.
///
/// XOR distance metric: `dist(a, b) = a XOR b` (byte-array lexicographic comparison).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// XOR distance from `other`.
    pub fn distance(&self, other: &NodeId) -> [u8; 32] {
        let mut d = [0u8; 32];
        for (i, byte) in d.iter_mut().enumerate() {
            *byte = self.0[i] ^ other.0[i];
        }
        d
    }

    /// Bucket index for a given XOR distance.
    ///
    /// Returns the index of the highest set bit in `dist` (0 = only the LSB set,
    /// 255 = MSB set, i.e. maximally far), or `None` for zero (same node).
    fn bucket_index(dist: &[u8; 32]) -> Option<usize> {
        for (byte_idx, &byte) in dist.iter().enumerate() {
            if byte != 0 {
                let leading = byte.leading_zeros() as usize;
                // Position of highest set bit within this byte.
                return Some(byte_idx * 8 + leading);
            }
        }
        None // all-zeros → same node, never stored
    }
}

// ── MailboxRecord ─────────────────────────────────────────────────────────────

struct MailboxRecord {
    addr: NodeAddr,
    expires_at: Instant,
}

// ── KBucket ───────────────────────────────────────────────────────────────────

/// A single Kademlia k-bucket: holds up to K peers sorted by last-seen time.
///
/// The head (index 0) is the least-recently seen entry; the tail is the most
/// recently seen.  When the bucket is full, the oldest entry is evicted.
struct KBucket {
    entries: Vec<(NodeId, NodeAddr)>,
}

impl KBucket {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Insert or refresh a peer.
    ///
    /// If the peer is already present, its address is updated and it is moved to
    /// the tail (most-recently-seen position).  If the bucket is full and the
    /// peer is new, the oldest entry (head) is evicted to make room.
    fn upsert(&mut self, id: NodeId, addr: NodeAddr) {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == id) {
            self.entries.remove(pos);
        } else if self.entries.len() >= K {
            self.entries.remove(0); // evict least-recently-seen
        }
        self.entries.push((id, addr));
    }

    fn iter(&self) -> impl Iterator<Item = &(NodeId, NodeAddr)> {
        self.entries.iter()
    }
}

// ── Dht ───────────────────────────────────────────────────────────────────────

/// DHT state: k-bucket routing table + mailbox record store.
///
/// Wrapped in `Arc` for shared ownership across handler tasks.
pub struct Dht {
    /// This node's DHT identifier (derived from its Ed25519 public key).
    pub node_id: NodeId,
    /// This node's network address (used when self-announcing).
    self_addr: Option<NodeAddr>,
    /// K-bucket routing table: 256 buckets, one per bit-prefix of XOR distance.
    routing_table: Mutex<Vec<KBucket>>,
    /// Mailbox records: user_pk → (addr, expiry).
    mailboxes: Arc<Mutex<HashMap<[u8; 32], MailboxRecord>>>,
}

impl Dht {
    /// Create a new DHT with the given node identity.
    ///
    /// `node_pk` is the node's Ed25519 public key (32 bytes), used as the DHT
    /// node ID and as the self-address for announcements.
    pub fn new(node_pk: [u8; 32], self_addr: Option<NodeAddr>) -> Self {
        let mut routing_table = Vec::with_capacity(256);
        for _ in 0..256 {
            routing_table.push(KBucket::new());
        }
        Self {
            node_id: NodeId(node_pk),
            self_addr,
            routing_table: Mutex::new(routing_table),
            mailboxes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return this node's own address (if configured).
    pub fn self_addr(&self) -> Option<&NodeAddr> {
        self.self_addr.as_ref()
    }

    // ── Routing table ─────────────────────────────────────────────────────

    /// Insert or refresh a remote peer in the routing table.
    ///
    /// No-op if `id` equals this node's own ID.
    pub fn add_peer(&self, id: NodeId, addr: NodeAddr) {
        if id == self.node_id {
            return;
        }
        let dist = self.node_id.distance(&id);
        if let Some(bucket_idx) = NodeId::bucket_index(&dist) {
            let mut rt = self.routing_table.lock().unwrap();
            rt[bucket_idx].upsert(id, addr);
        }
    }

    /// Return up to `count` peers closest to `target` by XOR distance.
    ///
    /// Used to populate `NodeResponse::ClosestPeers` responses.
    pub fn closest_peers(&self, target: &NodeId, count: usize) -> Vec<(NodeId, NodeAddr)> {
        let rt = self.routing_table.lock().unwrap();
        let mut candidates: Vec<(NodeId, NodeAddr, [u8; 32])> = rt
            .iter()
            .flat_map(|bucket| bucket.iter())
            .map(|(id, addr)| {
                let dist = target.distance(id);
                (*id, addr.clone(), dist)
            })
            .collect();
        candidates.sort_by_key(|(_, _, dist)| *dist);
        candidates
            .into_iter()
            .take(count)
            .map(|(id, addr, _)| (id, addr))
            .collect()
    }

    // ── Announcements ─────────────────────────────────────────────────────

    /// Announce that this node holds the mailbox for `user_pk`.
    ///
    /// Overwrites any existing record (resets TTL).
    /// No-op if `self_addr` was not configured.
    pub fn announce_mailbox(&self, user_pk: [u8; 32]) {
        if let Some(addr) = self.self_addr.clone() {
            self.mailboxes.lock().unwrap().insert(
                user_pk,
                MailboxRecord {
                    addr,
                    expires_at: Instant::now() + MAILBOX_TTL,
                },
            );
        }
    }

    // ── Lookups ───────────────────────────────────────────────────────────

    /// Return the NodeAddr holding the mailbox for `user_pk`, if known and not expired.
    pub fn lookup_mailbox(&self, user_pk: &[u8; 32]) -> Option<NodeAddr> {
        let mailboxes = self.mailboxes.lock().unwrap();
        mailboxes.get(user_pk).and_then(|rec| {
            if rec.expires_at > Instant::now() {
                Some(rec.addr.clone())
            } else {
                None
            }
        })
    }

    // ── External record injection ─────────────────────────────────────────

    /// Inject a mailbox record received from another node via `StoreDhtRecord`.
    pub fn add_mailbox_record(&self, user_pk: [u8; 32], addr: NodeAddr) {
        self.mailboxes.lock().unwrap().insert(
            user_pk,
            MailboxRecord {
                addr,
                expires_at: Instant::now() + MAILBOX_TTL,
            },
        );
    }

    // ── Expiry ────────────────────────────────────────────────────────────

    /// Remove all expired mailbox records.
    ///
    /// Should be called periodically (e.g., every 10 minutes) to free memory.
    pub fn expire_records(&self) {
        let now = Instant::now();
        self.mailboxes
            .lock()
            .unwrap()
            .retain(|_, rec| rec.expires_at > now);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr(port: u16) -> NodeAddr {
        NodeAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn announce_and_lookup_mailbox() {
        let dht = Dht::new([1u8; 32], Some(addr(9002)));
        let user_pk = [4u8; 32];
        dht.announce_mailbox(user_pk);
        assert_eq!(dht.lookup_mailbox(&user_pk).unwrap().port, 9002);
    }

    #[test]
    fn mailbox_overwritten_on_re_announce() {
        let dht = Dht::new([1u8; 32], Some(addr(9001)));
        let user_pk = [3u8; 32];
        dht.announce_mailbox(user_pk);
        // Simulate a new node addr taking over the mailbox.
        dht.add_mailbox_record(user_pk, addr(9999));
        assert_eq!(dht.lookup_mailbox(&user_pk).unwrap().port, 9999);
    }

    #[test]
    fn external_mailbox_record_injected() {
        let dht = Dht::new([1u8; 32], None);
        let user_pk = [5u8; 32];
        dht.add_mailbox_record(user_pk, addr(7000));
        assert_eq!(dht.lookup_mailbox(&user_pk).unwrap().port, 7000);
    }

    #[test]
    fn xor_distance_is_symmetric() {
        let a = NodeId([0u8; 32]);
        let mut b_bytes = [0u8; 32];
        b_bytes[0] = 0xFF;
        let b = NodeId(b_bytes);
        assert_eq!(a.distance(&b), b.distance(&a));
    }

    #[test]
    fn k_bucket_evicts_oldest_when_full() {
        let self_pk = [0u8; 32];
        let dht = Dht::new(self_pk, None);
        // Add K+1 peers all mapped to bucket 0.
        // XOR distance [0xFF, *, *, ...] always has leading_zeros(0xFF)=0 in
        // byte 0, so bucket_index = 0*8+0 = 0 regardless of the remaining bytes.
        for i in 0u8..=(K as u8) {
            let mut id = [0xFFu8; 32];
            id[31] = i; // unique IDs, all in bucket 0
            dht.add_peer(NodeId(id), addr(9000 + i as u16));
        }
        // Bucket 0 must hold at most K entries after eviction.
        let rt = dht.routing_table.lock().unwrap();
        let bucket_0_count = rt[0].entries.len();
        assert!(
            bucket_0_count <= K,
            "bucket 0 exceeded K entries: {bucket_0_count}"
        );
    }

    #[test]
    fn closest_peers_returns_sorted_by_xor() {
        let self_pk = [0u8; 32];
        let dht = Dht::new(self_pk, None);
        // Add 3 peers at different XOR distances from the target [1,0,0,...].
        let target = NodeId([
            1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let near_id = [
            1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 1,
        ]; // dist=1 in last byte
        let far_id = [0xFFu8; 32]; // very far
        let mid_id = [
            3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]; // dist=2 in first byte
        dht.add_peer(NodeId(near_id), addr(9001));
        dht.add_peer(NodeId(far_id), addr(9003));
        dht.add_peer(NodeId(mid_id), addr(9002));
        let closest = dht.closest_peers(&target, 3);
        assert_eq!(closest.len(), 3);
        // Closest peer should have smallest XOR distance to target.
        let d0 = target.distance(&closest[0].0);
        let d1 = target.distance(&closest[1].0);
        assert!(d0 <= d1, "peers not sorted by distance");
    }

    #[test]
    fn expired_records_are_removed() {
        // We can't easily mock Instant, so we test expire_records on live records.
        let dht = Dht::new([1u8; 32], Some(addr(9001)));
        let user_pk = [7u8; 32];
        // Inject a record that expires "instantly" by manually inserting with past expiry.
        dht.mailboxes.lock().unwrap().insert(
            user_pk,
            MailboxRecord {
                addr: addr(9001),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
        assert!(
            dht.lookup_mailbox(&user_pk).is_none(),
            "expired record should not be found"
        );
        dht.expire_records();
        assert!(
            dht.mailboxes.lock().unwrap().is_empty(),
            "expired record not removed by expire_records"
        );
    }
}
