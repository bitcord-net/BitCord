//! Persistent DHT record storage backed by `redb`.
//!
//! Stores community peer records and peer info records so that
//! DHT knowledge survives node restarts.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};

use crate::routing::{CommunityPeerRecord, PeerInfoRecord};

/// `dht_community_peers`: key = `community_pk(32) ++ node_pk(32)` = 64 bytes
///                        value = `postcard(CommunityPeerRecord)`
const DHT_PEERS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("dht_community_peers");

/// `dht_peer_info`: key = `peer_id(32 bytes)`, value = `postcard(PeerInfoRecord)`
const DHT_PEER_INFO: TableDefinition<&[u8], &[u8]> = TableDefinition::new("dht_peer_info");

fn dht_peer_key(community_pk: &[u8; 32], node_pk: &[u8; 32]) -> [u8; 64] {
    let mut k = [0u8; 64];
    k[..32].copy_from_slice(community_pk);
    k[32..].copy_from_slice(node_pk);
    k
}

/// Thread-safe persistent store for DHT community peer records and peer info records.
pub struct DhtStore {
    db: Arc<Database>,
}

impl DhtStore {
    /// Open (or create) the DHT database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).context("open DHT redb database")?;
        let wtxn = db.begin_write().context("init write transaction")?;
        {
            wtxn.open_table(DHT_PEERS)
                .context("init dht_community_peers table")?;
            wtxn.open_table(DHT_PEER_INFO)
                .context("init dht_peer_info table")?;
        }
        wtxn.commit().context("commit init transaction")?;
        Ok(Self { db: Arc::new(db) })
    }

    /// Persist a community peer record.
    pub fn set_community_peer_record(
        &self,
        community_pk: &[u8; 32],
        record: &CommunityPeerRecord,
    ) -> Result<()> {
        let key = dht_peer_key(community_pk, &record.node_pk);
        let value = postcard::to_allocvec(record).context("encode CommunityPeerRecord")?;
        let wtxn = self.db.begin_write()?;
        {
            let mut table = wtxn.open_table(DHT_PEERS)?;
            table.insert(key.as_slice(), value.as_slice())?;
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Return all stored community peer records as `(community_pk, record)` pairs.
    pub fn all_community_peer_records(&self) -> Result<Vec<([u8; 32], CommunityPeerRecord)>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(DHT_PEERS)?;
        let mut result = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            if k.value().len() < 64 {
                continue;
            }
            let mut community_pk = [0u8; 32];
            community_pk.copy_from_slice(&k.value()[..32]);
            match postcard::from_bytes::<CommunityPeerRecord>(v.value()) {
                Ok(record) => result.push((community_pk, record)),
                Err(e) => {
                    tracing::warn!(
                        "DHT: skipping unreadable community peer record (schema migration?): {e}"
                    );
                }
            }
        }
        Ok(result)
    }

    /// Remove all community peer records older than `cutoff_secs` Unix timestamp.
    pub fn remove_expired_community_peers(&self, cutoff_secs: u64) -> Result<()> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(DHT_PEERS)?;
        let expired_keys: Vec<Vec<u8>> = table
            .iter()?
            .filter_map(|item| {
                let (k, v) = item.ok()?;
                let record: CommunityPeerRecord = postcard::from_bytes(v.value()).ok()?;
                if record.announced_at < cutoff_secs {
                    Some(k.value().to_vec())
                } else {
                    None
                }
            })
            .collect();
        drop(table);
        drop(rtxn);

        if expired_keys.is_empty() {
            return Ok(());
        }
        let wtxn = self.db.begin_write()?;
        {
            let mut table = wtxn.open_table(DHT_PEERS)?;
            for key in &expired_keys {
                table.remove(key.as_slice())?;
            }
        }
        wtxn.commit()?;
        Ok(())
    }

    // ── Peer info records ─────────────────────────────────────────────────

    /// Persist a peer info record keyed by `peer_id`.
    pub fn set_peer_info_record(&self, peer_id: &[u8; 32], record: &PeerInfoRecord) -> Result<()> {
        let value = postcard::to_allocvec(record).context("encode PeerInfoRecord")?;
        let wtxn = self.db.begin_write()?;
        {
            let mut table = wtxn.open_table(DHT_PEER_INFO)?;
            table.insert(peer_id.as_slice(), value.as_slice())?;
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Load a single peer info record by `peer_id`.
    pub fn get_peer_info_record(&self, peer_id: &[u8; 32]) -> Result<Option<PeerInfoRecord>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(DHT_PEER_INFO)?;
        match table.get(peer_id.as_slice())? {
            Some(v) => Ok(Some(
                postcard::from_bytes::<PeerInfoRecord>(v.value())
                    .context("decode PeerInfoRecord")?,
            )),
            None => Ok(None),
        }
    }

    /// Return all stored peer info records as `(peer_id, record)` pairs.
    pub fn all_peer_info_records(&self) -> Result<Vec<([u8; 32], PeerInfoRecord)>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(DHT_PEER_INFO)?;
        let mut result = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            if k.value().len() != 32 {
                continue;
            }
            let mut peer_id = [0u8; 32];
            peer_id.copy_from_slice(k.value());
            match postcard::from_bytes::<PeerInfoRecord>(v.value()) {
                Ok(record) => result.push((peer_id, record)),
                Err(e) => {
                    tracing::warn!(
                        "DHT: skipping unreadable peer info record (schema migration?): {e}"
                    );
                }
            }
        }
        Ok(result)
    }

    /// Remove all peer info records older than `cutoff_secs` Unix timestamp.
    pub fn remove_expired_peer_infos(&self, cutoff_secs: u64) -> Result<()> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(DHT_PEER_INFO)?;
        let expired_keys: Vec<Vec<u8>> = table
            .iter()?
            .filter_map(|item| {
                let (k, v) = item.ok()?;
                let record: PeerInfoRecord = postcard::from_bytes(v.value()).ok()?;
                if record.announced_at < cutoff_secs {
                    Some(k.value().to_vec())
                } else {
                    None
                }
            })
            .collect();
        drop(table);
        drop(rtxn);

        if expired_keys.is_empty() {
            return Ok(());
        }
        let wtxn = self.db.begin_write()?;
        {
            let mut table = wtxn.open_table(DHT_PEER_INFO)?;
            for key in &expired_keys {
                table.remove(key.as_slice())?;
            }
        }
        wtxn.commit()?;
        Ok(())
    }
}
