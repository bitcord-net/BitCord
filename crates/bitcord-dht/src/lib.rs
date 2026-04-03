//! `bitcord-dht` — standalone Kademlia DHT data layer.
//!
//! This crate provides:
//! - [`NodeAddr`] — lightweight IP+port type used across the network stack.
//! - [`NodeId`] — 256-bit DHT node identifier (XOR metric).
//! - [`CommunityPeerRecord`] — a peer record for community peer discovery.
//! - [`DhtState`] — in-memory k-bucket routing table + mailbox + community peer store.
//! - [`DhtStore`] — redb-backed persistence for DHT records.
//! - [`spawn_expiry_task`] — background TTL expiry task.
//!
//! This crate has **no dependency on `bitcord-core`**.  The QUIC-based Kademlia
//! iterative lookup lives in `bitcord-core::dht` which wraps `DhtState` with
//! outbound QUIC connections.

pub mod addr;
pub mod expiry;
pub mod routing;
pub mod store;

pub use addr::NodeAddr;
pub use expiry::spawn_expiry_task;
pub use routing::{CommunityPeerRecord, DhtState, NodeId};
pub use store::DhtStore;
