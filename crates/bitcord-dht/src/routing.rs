//! Lightweight DHT for mailbox routing and community peer discovery.
//!
//! # Design
//! Maintains a **k-bucket routing table** (K=20, Kademlia-style XOR metric) so
//! that the node can route `FIND_NODE` queries across the network, combined with
//! a mailbox record store and community peer store with TTL-based expiry.
//!
//! # Record types
//! - `mailbox/<user_pk>` → `(NodeAddr, expires_at)` — which node holds a user's mailbox
//! - `community_peers/<community_pk>` → `[(node_pk, NodeAddr, announced_at)]` — peers in a community
//! - `peer_info/<peer_id>` → `(x25519_pk, NodeAddr, announced_at)` — peer's encryption key and address

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::addr::NodeAddr;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of peers per k-bucket (standard Kademlia K value).
pub const K: usize = 20;

/// Mailbox record TTL: 24 hours.
const MAILBOX_TTL: Duration = Duration::from_secs(86_400);

/// Community peer record TTL: 1 hour (in seconds).
pub const COMMUNITY_PEER_TTL_SECS: u64 = 3600;

/// Peer info record TTL: 24 hours.
pub const PEER_INFO_TTL_SECS: u64 = 86_400;

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
    fn bucket_index(dist: &[u8; 32]) -> Option<usize> {
        for (byte_idx, &byte) in dist.iter().enumerate() {
            if byte != 0 {
                let leading = byte.leading_zeros() as usize;
                return Some(255 - (byte_idx * 8 + leading));
            }
        }
        None
    }
}

// ── MailboxRecord ─────────────────────────────────────────────────────────────

struct MailboxRecord {
    addr: NodeAddr,
    expires_at: Instant,
}

// ── CommunityPeerRecord ───────────────────────────────────────────────────────

/// A record of a single peer known to be a member of a community.
///
/// `announced_at` is a Unix timestamp (seconds) so the record can be
/// serialised to disk and reloaded across restarts.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CommunityPeerRecord {
    /// Ed25519 public key of the peer node.
    pub node_pk: [u8; 32],
    /// Primary (public) network address where the peer is reachable.
    pub addr: NodeAddr,
    /// Unix timestamp (seconds) when this record was last announced.
    pub announced_at: u64,
}

// ── PeerInfoRecord ────────────────────────────────────────────────────────────

/// A record advertising a peer's X25519 encryption key and current QUIC address.
///
/// Keyed by `peer_id` (SHA-256 of the peer's Ed25519 verifying key).
/// `announced_at` is a Unix timestamp (seconds) for TTL-based expiry.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerInfoRecord {
    /// X25519 public key for DM envelope encryption.
    pub x25519_pk: [u8; 32],
    /// Primary (public) network address where the peer is reachable.
    pub addr: NodeAddr,
    /// Unix timestamp (seconds) when this record was last announced.
    pub announced_at: u64,
    /// Human-readable display name chosen by the peer.
    #[serde(default)]
    pub display_name: String,
}

// ── KBucket ───────────────────────────────────────────────────────────────────

struct KBucket {
    entries: Vec<(NodeId, NodeAddr)>,
}

impl KBucket {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn upsert(&mut self, id: NodeId, addr: NodeAddr) {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == id) {
            self.entries.remove(pos);
        } else if self.entries.len() >= K {
            self.entries.remove(0);
        }
        self.entries.push((id, addr));
    }

    fn iter(&self) -> impl Iterator<Item = &(NodeId, NodeAddr)> {
        self.entries.iter()
    }
}

// ── DhtState ──────────────────────────────────────────────────────────────────

/// In-memory DHT state: k-bucket routing table + mailbox records + community peer records.
///
/// Wrapped in `Arc` for shared ownership across handler tasks.
pub struct DhtState {
    /// This node's DHT identifier.
    pub node_id: NodeId,
    /// This node's network address (set after STUN/UPnP discovery).
    self_addr: Mutex<Option<NodeAddr>>,
    /// K-bucket routing table: 256 buckets.
    routing_table: Mutex<Vec<KBucket>>,
    /// Mailbox records: user_pk → (addr, expiry).
    mailboxes: Arc<Mutex<HashMap<[u8; 32], MailboxRecord>>>,
    /// Community peer records: community_pk → list of known peer records.
    community_peers: Mutex<HashMap<[u8; 32], Vec<CommunityPeerRecord>>>,
    /// Peer info records: peer_id → PeerInfoRecord (x25519_pk + addr).
    peer_infos: Mutex<HashMap<[u8; 32], PeerInfoRecord>>,
    /// Last time `DiscoverAndDial` was run per community_pk.
    last_discover: Mutex<HashMap<[u8; 32], Instant>>,
    /// Last time community presence was announced per community_pk.
    last_announce: Mutex<HashMap<[u8; 32], Instant>>,
}

impl DhtState {
    pub fn new(node_pk: [u8; 32], self_addr: Option<NodeAddr>) -> Self {
        let mut routing_table = Vec::with_capacity(256);
        for _ in 0..256 {
            routing_table.push(KBucket::new());
        }
        Self {
            node_id: NodeId(node_pk),
            self_addr: Mutex::new(self_addr),
            routing_table: Mutex::new(routing_table),
            mailboxes: Arc::new(Mutex::new(HashMap::new())),
            community_peers: Mutex::new(HashMap::new()),
            peer_infos: Mutex::new(HashMap::new()),
            last_discover: Mutex::new(HashMap::new()),
            last_announce: Mutex::new(HashMap::new()),
        }
    }

    pub fn self_addr(&self) -> Option<NodeAddr> {
        self.self_addr.lock().unwrap().clone()
    }

    pub fn set_self_addr(&self, addr: NodeAddr) {
        *self.self_addr.lock().unwrap() = Some(addr);
    }

    // ── Cooldowns ─────────────────────────────────────────────────────────

    pub fn acquire_discover_slot(&self, community_pk: [u8; 32], cooldown: Duration) -> bool {
        let mut map = self.last_discover.lock().unwrap();
        let now = Instant::now();
        if map
            .get(&community_pk)
            .is_none_or(|t| now.duration_since(*t) >= cooldown)
        {
            map.insert(community_pk, now);
            true
        } else {
            false
        }
    }

    pub fn acquire_announce_slot(&self, community_pk: [u8; 32], cooldown: Duration) -> bool {
        let mut map = self.last_announce.lock().unwrap();
        let now = Instant::now();
        if map
            .get(&community_pk)
            .is_none_or(|t| now.duration_since(*t) >= cooldown)
        {
            map.insert(community_pk, now);
            true
        } else {
            false
        }
    }

    // ── Routing table ─────────────────────────────────────────────────────

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

    // ── Mailbox records ───────────────────────────────────────────────────

    /// Announce that this node holds the mailbox for `user_pk`.
    pub fn announce_mailbox(&self, user_pk: [u8; 32]) {
        if let Some(addr) = self.self_addr() {
            self.mailboxes.lock().unwrap().insert(
                user_pk,
                MailboxRecord {
                    addr,
                    expires_at: Instant::now() + MAILBOX_TTL,
                },
            );
        }
    }

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

    pub fn add_mailbox_record(&self, user_pk: [u8; 32], addr: NodeAddr) {
        self.mailboxes.lock().unwrap().insert(
            user_pk,
            MailboxRecord {
                addr,
                expires_at: Instant::now() + MAILBOX_TTL,
            },
        );
    }

    // ── Community peer records ────────────────────────────────────────────

    pub fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn announce_community_peer(
        &self,
        community_pk: [u8; 32],
        node_pk: [u8; 32],
        addr: NodeAddr,
    ) {
        let now = Self::unix_now();
        let mut peers = self.community_peers.lock().unwrap();
        let list = peers.entry(community_pk).or_default();
        if let Some(existing) = list.iter_mut().find(|r| r.node_pk == node_pk) {
            existing.addr = addr;
            existing.announced_at = now;
        } else {
            list.push(CommunityPeerRecord {
                node_pk,
                addr,
                announced_at: now,
            });
            if list.len() > K {
                list.sort_by(|a, b| b.announced_at.cmp(&a.announced_at));
                list.truncate(K);
            }
        }
    }

    pub fn lookup_community_peers(&self, community_pk: &[u8; 32]) -> Vec<CommunityPeerRecord> {
        let now = Self::unix_now();
        let peers = self.community_peers.lock().unwrap();
        peers
            .get(community_pk)
            .map(|list| {
                list.iter()
                    .filter(|r| now.saturating_sub(r.announced_at) < COMMUNITY_PEER_TTL_SECS)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn add_community_peer_record(&self, community_pk: [u8; 32], record: CommunityPeerRecord) {
        let mut peers = self.community_peers.lock().unwrap();
        let list = peers.entry(community_pk).or_default();
        if let Some(existing) = list.iter_mut().find(|r| r.node_pk == record.node_pk) {
            *existing = record;
        } else {
            list.push(record);
        }
    }

    pub fn all_community_peers(&self) -> HashMap<[u8; 32], Vec<CommunityPeerRecord>> {
        self.community_peers.lock().unwrap().clone()
    }

    // ── Peer info records ─────────────────────────────────────────────────

    /// Announce this node's own peer info (x25519_pk + addr + display_name), keyed by `peer_id`.
    pub fn announce_peer_info(
        &self,
        peer_id: [u8; 32],
        x25519_pk: [u8; 32],
        addr: NodeAddr,
        display_name: String,
    ) {
        let now = Self::unix_now();
        self.peer_infos.lock().unwrap().insert(
            peer_id,
            PeerInfoRecord {
                x25519_pk,
                addr,
                announced_at: now,
                display_name,
            },
        );
    }

    /// Look up a cached peer info record by `peer_id`.
    pub fn lookup_peer_info(&self, peer_id: &[u8; 32]) -> Option<PeerInfoRecord> {
        let now = Self::unix_now();
        let infos = self.peer_infos.lock().unwrap();
        infos.get(peer_id).and_then(|rec| {
            if now.saturating_sub(rec.announced_at) < PEER_INFO_TTL_SECS {
                Some(rec.clone())
            } else {
                None
            }
        })
    }

    /// Inject a peer info record received from another node.
    pub fn add_peer_info_record(&self, peer_id: [u8; 32], record: PeerInfoRecord) {
        self.peer_infos.lock().unwrap().insert(peer_id, record);
    }

    // ── Expiry ────────────────────────────────────────────────────────────

    pub fn expire_records(&self) {
        let now = Instant::now();
        self.mailboxes
            .lock()
            .unwrap()
            .retain(|_, rec| rec.expires_at > now);
        self.expire_community_peers();
        self.expire_peer_infos();
    }

    fn expire_community_peers(&self) {
        let now = Self::unix_now();
        let mut peers = self.community_peers.lock().unwrap();
        for list in peers.values_mut() {
            list.retain(|r| now.saturating_sub(r.announced_at) < COMMUNITY_PEER_TTL_SECS);
        }
        peers.retain(|_, list| !list.is_empty());
    }

    fn expire_peer_infos(&self) {
        let now = Self::unix_now();
        self.peer_infos
            .lock()
            .unwrap()
            .retain(|_, rec| now.saturating_sub(rec.announced_at) < PEER_INFO_TTL_SECS);
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
        let dht = DhtState::new([1u8; 32], Some(addr(9002)));
        let user_pk = [4u8; 32];
        dht.announce_mailbox(user_pk);
        assert_eq!(dht.lookup_mailbox(&user_pk).unwrap().port, 9002);
    }

    #[test]
    fn mailbox_overwritten_on_re_announce() {
        let dht = DhtState::new([1u8; 32], Some(addr(9001)));
        let user_pk = [3u8; 32];
        dht.announce_mailbox(user_pk);
        dht.add_mailbox_record(user_pk, addr(9999));
        assert_eq!(dht.lookup_mailbox(&user_pk).unwrap().port, 9999);
    }

    #[test]
    fn external_mailbox_record_injected() {
        let dht = DhtState::new([1u8; 32], None);
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
        let dht = DhtState::new(self_pk, None);
        for i in 0u8..=(K as u8) {
            let mut id = [0xFFu8; 32];
            id[31] = i;
            dht.add_peer(NodeId(id), addr(9000 + i as u16));
        }
        let rt = dht.routing_table.lock().unwrap();
        let bucket_255_count = rt[255].entries.len();
        assert!(bucket_255_count <= K);
    }

    #[test]
    fn closest_peers_returns_sorted_by_xor() {
        let self_pk = [0u8; 32];
        let dht = DhtState::new(self_pk, None);
        let target = NodeId([
            1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let near_id = [
            1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 1,
        ];
        let far_id = [0xFFu8; 32];
        let mid_id = [
            3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];
        dht.add_peer(NodeId(near_id), addr(9001));
        dht.add_peer(NodeId(far_id), addr(9003));
        dht.add_peer(NodeId(mid_id), addr(9002));
        let closest = dht.closest_peers(&target, 3);
        assert_eq!(closest.len(), 3);
        let d0 = target.distance(&closest[0].0);
        let d1 = target.distance(&closest[1].0);
        assert!(d0 <= d1);
    }

    #[test]
    fn expired_records_are_removed() {
        let dht = DhtState::new([1u8; 32], Some(addr(9001)));
        let user_pk = [7u8; 32];
        dht.mailboxes.lock().unwrap().insert(
            user_pk,
            MailboxRecord {
                addr: addr(9001),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
        assert!(dht.lookup_mailbox(&user_pk).is_none());
        dht.expire_records();
        assert!(dht.mailboxes.lock().unwrap().is_empty());
    }

    #[test]
    fn announce_and_lookup_community_peers() {
        let dht = DhtState::new([1u8; 32], None);
        let community_pk = [10u8; 32];
        dht.announce_community_peer(community_pk, [20u8; 32], addr(9100));
        dht.announce_community_peer(community_pk, [21u8; 32], addr(9101));
        let records = dht.lookup_community_peers(&community_pk);
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn community_peer_list_capped_at_k() {
        let dht = DhtState::new([1u8; 32], None);
        let community_pk = [12u8; 32];
        for i in 0u8..=(K as u8 + 4) {
            let mut node_pk = [0u8; 32];
            node_pk[31] = i;
            dht.announce_community_peer(community_pk, node_pk, addr(9300 + i as u16));
        }
        let records = dht.lookup_community_peers(&community_pk);
        assert!(records.len() <= K);
    }

    // ── PeerInfoRecord tests ──────────────────────────────────────────────────

    #[test]
    fn announce_and_lookup_peer_info() {
        let dht = DhtState::new([1u8; 32], None);
        let peer_id = [42u8; 32];
        let x25519_pk = [7u8; 32];
        dht.announce_peer_info(peer_id, x25519_pk, addr(9200), "Alice".to_string());
        let rec = dht
            .lookup_peer_info(&peer_id)
            .expect("record should be present");
        assert_eq!(rec.x25519_pk, x25519_pk);
        assert_eq!(rec.addr.port, 9200);
        assert_eq!(rec.display_name, "Alice");
    }

    #[test]
    fn peer_info_overwritten_on_re_announce() {
        let dht = DhtState::new([1u8; 32], None);
        let peer_id = [43u8; 32];
        dht.announce_peer_info(peer_id, [1u8; 32], addr(9201), "OldName".to_string());
        dht.announce_peer_info(peer_id, [2u8; 32], addr(9202), "NewName".to_string());
        let rec = dht
            .lookup_peer_info(&peer_id)
            .expect("record should be present");
        assert_eq!(rec.x25519_pk, [2u8; 32]);
        assert_eq!(rec.addr.port, 9202);
        assert_eq!(rec.display_name, "NewName");
    }

    #[test]
    fn peer_info_add_record_then_lookup() {
        let dht = DhtState::new([1u8; 32], None);
        let peer_id = [44u8; 32];
        let record = PeerInfoRecord {
            x25519_pk: [9u8; 32],
            addr: addr(9203),
            announced_at: DhtState::unix_now(),
            display_name: "Bob".to_string(),
        };
        dht.add_peer_info_record(peer_id, record);
        let rec = dht
            .lookup_peer_info(&peer_id)
            .expect("injected record should be present");
        assert_eq!(rec.x25519_pk, [9u8; 32]);
        assert_eq!(rec.display_name, "Bob");
    }

    #[test]
    fn peer_info_unknown_returns_none() {
        let dht = DhtState::new([1u8; 32], None);
        assert!(dht.lookup_peer_info(&[99u8; 32]).is_none());
    }

    #[test]
    fn expired_peer_info_removed_by_expire_records() {
        let dht = DhtState::new([1u8; 32], None);
        let peer_id = [45u8; 32];
        // Insert a record with an announced_at that is already beyond the TTL.
        let expired_at = DhtState::unix_now().saturating_sub(PEER_INFO_TTL_SECS + 1);
        dht.add_peer_info_record(
            peer_id,
            PeerInfoRecord {
                x25519_pk: [3u8; 32],
                addr: addr(9204),
                announced_at: expired_at,
                display_name: String::new(),
            },
        );
        // lookup_peer_info should already suppress the expired record...
        assert!(
            dht.lookup_peer_info(&peer_id).is_none(),
            "expired record should not be returned by lookup"
        );
        // ...and expire_records should physically remove it.
        dht.expire_records();
        assert!(
            dht.peer_infos.lock().unwrap().is_empty(),
            "expired record should be removed by expire_records"
        );
    }

    #[test]
    fn peer_info_round_trips_via_postcard() {
        // Serialize a PeerInfoRecord through postcard (the wire format used by the DHT store)
        // and verify it round-trips cleanly, including the display_name field.
        let original = PeerInfoRecord {
            x25519_pk: [5u8; 32],
            addr: addr(9205),
            announced_at: 1_700_000_000,
            display_name: "Charlie".to_string(),
        };
        let bytes = postcard::to_allocvec(&original).expect("serialize");
        let decoded: PeerInfoRecord = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(decoded.x25519_pk, original.x25519_pk);
        assert_eq!(decoded.addr.port, original.addr.port);
        assert_eq!(decoded.announced_at, original.announced_at);
        assert_eq!(decoded.display_name, "Charlie");
    }
}
