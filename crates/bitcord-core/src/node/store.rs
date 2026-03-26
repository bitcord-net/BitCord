//! Persistent node storage backed by `redb`.
//!
//! Three tables:
//! - `community_log`  — append-only per-channel message log
//! - `mailboxes`      — per-user DM mailbox
//! - `community_meta` — hosting cert + channel manifests per community
//!
//! Key encoding uses fixed-size byte arrays so lexicographic ordering gives
//! correct range scans.

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use ulid::Ulid;

use crate::{
    crypto::{certificate::HostingCert, dm::DmEnvelope},
    model::{channel::ChannelManifest, community::SignedManifest, membership::MembershipRecord},
    state::message_log::LogEntry,
};
use std::collections::HashMap;

// ── Table definitions ─────────────────────────────────────────────────────────

/// `community_log`: key = `community_pk(32) ++ channel_ulid_be(16) ++ seq_be(8)` = 56 bytes
///                  value = `postcard(LogEntry)`
const LOG: TableDefinition<&[u8], &[u8]> = TableDefinition::new("community_log");

/// `mailboxes`: key = `user_pk(32) ++ seq_be(8)` = 40 bytes
///              value = `postcard(LogEntry)`
const MAIL: TableDefinition<&[u8], &[u8]> = TableDefinition::new("mailboxes");

/// `community_meta`: key = `community_pk(32)`
///                   value = `postcard(CommunityMeta)`
const META: TableDefinition<&[u8], &[u8]> = TableDefinition::new("community_meta");

// ── Supporting types ──────────────────────────────────────────────────────────

/// Metadata stored per community on this node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommunityMeta {
    /// Hosting certificate that authorised this community on this node.
    pub cert: HostingCert,
    /// The full signed community manifest (name, description, etc).
    pub manifest: Option<SignedManifest>,
    /// Known channel manifests (populated as channel metadata arrives).
    pub channels: Vec<ChannelManifest>,
    /// Per-channel symmetric key material (plaintext pre-E2EE).
    pub channel_keys: HashMap<String, Vec<u8>>,
    /// Membership records for this community, keyed by user_id hex string.
    pub members: HashMap<String, MembershipRecord>,
}

// ── Key encoding helpers ──────────────────────────────────────────────────────

fn ulid_be(id: &Ulid) -> [u8; 16] {
    id.0.to_be_bytes()
}

/// `community_pk(32) ++ channel_ulid_be(16) ++ seq_be(8)` = 56 bytes
fn log_key(community_pk: &[u8; 32], chan: &[u8; 16], seq: u64) -> [u8; 56] {
    let mut k = [0u8; 56];
    k[..32].copy_from_slice(community_pk);
    k[32..48].copy_from_slice(chan);
    k[48..].copy_from_slice(&seq.to_be_bytes());
    k
}

/// `user_pk(32) ++ seq_be(8)` = 40 bytes
fn mail_key(user_pk: &[u8; 32], seq: u64) -> [u8; 40] {
    let mut k = [0u8; 40];
    k[..32].copy_from_slice(user_pk);
    k[32..].copy_from_slice(&seq.to_be_bytes());
    k
}

// ── NodeStore ─────────────────────────────────────────────────────────────────

/// Thread-safe persistent store for a BitCord node.
///
/// Backed by `redb` — a transactional embedded key-value database.
/// Designed to be wrapped in `Arc` and shared across handler tasks.
pub struct NodeStore {
    db: Arc<Database>,
}

impl NodeStore {
    /// Open (or create) the node database at `path`.
    ///
    /// Initialises all tables if they do not yet exist.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path).context("open redb database")?;
        let wtxn = db.begin_write().context("init write transaction")?;
        {
            wtxn.open_table(LOG).context("init community_log table")?;
            wtxn.open_table(MAIL).context("init mailboxes table")?;
            wtxn.open_table(META).context("init community_meta table")?;
        }
        wtxn.commit().context("commit init transaction")?;
        Ok(Self { db: Arc::new(db) })
    }

    // ── Community message log ─────────────────────────────────────────────

    /// Append an encrypted message to a community channel log.
    ///
    /// Returns the assigned sequence number (0-based, monotonically increasing
    /// per `(community_pk, channel_id)` pair).
    #[allow(clippy::too_many_arguments)]
    pub fn append_message(
        &self,
        community_pk: &[u8; 32],
        channel_id: &Ulid,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
        message_id: String,
        author_id: String,
        timestamp_ms: i64,
    ) -> Result<u64> {
        let chan = ulid_be(channel_id);
        let wtxn = self.db.begin_write()?;
        let seq = {
            let mut table = wtxn.open_table(LOG)?;
            // Find the highest existing seq for this channel by scanning backwards.
            let lower = log_key(community_pk, &chan, 0);
            let upper = log_key(community_pk, &chan, u64::MAX);
            let seq = match table
                .range(lower.as_slice()..=upper.as_slice())?
                .next_back()
            {
                Some(Ok((k, _))) if k.value().len() >= 56 => {
                    u64::from_be_bytes(k.value()[48..56].try_into().unwrap()) + 1
                }
                _ => 0,
            };
            let entry = LogEntry {
                seq,
                nonce,
                ciphertext,
                message_id,
                author_id,
                timestamp_ms,
                deleted: false,
            };
            let key = log_key(community_pk, &chan, seq);
            let value = postcard::to_allocvec(&entry).context("encode LogEntry")?;
            table.insert(key.as_slice(), value.as_slice())?;
            seq
        };
        wtxn.commit()?;
        Ok(seq)
    }

    /// Return the highest sequence number stored for a channel, or `None` if empty.
    pub fn last_seq(&self, community_pk: &[u8; 32], channel_id: &Ulid) -> Result<Option<u64>> {
        let chan = ulid_be(channel_id);
        let lower = log_key(community_pk, &chan, 0);
        let upper = log_key(community_pk, &chan, u64::MAX);
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(LOG)?;
        let last = table
            .range(lower.as_slice()..=upper.as_slice())?
            .next_back()
            .transpose()?
            .and_then(|(k, _)| {
                if k.value().len() >= 56 {
                    Some(u64::from_be_bytes(k.value()[48..56].try_into().unwrap()))
                } else {
                    None
                }
            });
        Ok(last)
    }

    /// Return all channel log entries with `seq >= since_seq`.
    pub fn get_messages(
        &self,
        community_pk: &[u8; 32],
        channel_id: &Ulid,
        since_seq: u64,
    ) -> Result<Vec<LogEntry>> {
        let chan = ulid_be(channel_id);
        let lower = log_key(community_pk, &chan, since_seq);
        let upper = log_key(community_pk, &chan, u64::MAX);
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(LOG)?;
        let mut entries = Vec::new();
        for item in table.range(lower.as_slice()..=upper.as_slice())? {
            let (_, v) = item?;
            entries.push(postcard::from_bytes::<LogEntry>(v.value()).context("decode LogEntry")?);
        }
        Ok(entries)
    }

    // ── DM mailboxes ──────────────────────────────────────────────────────

    /// Store a `DmEnvelope` in the recipient's mailbox.
    ///
    /// The envelope is serialised into a `LogEntry`'s ciphertext field so the
    /// mailbox uses the same log-entry type as channel messages.
    ///
    /// Returns the assigned sequence number in the recipient's mailbox.
    pub fn append_dm(
        &self,
        recipient_pk: &[u8; 32],
        sender_pk: &[u8; 32],
        envelope: &DmEnvelope,
    ) -> Result<u64> {
        let ciphertext = postcard::to_allocvec(envelope).context("encode DmEnvelope")?;
        // Derive the canonical peer_id: SHA-256 of the Ed25519 verifying key,
        // matching UserId::from_verifying_key / NodeIdentity::to_peer_id.
        let author_id: String = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(sender_pk);
            hash.iter().map(|b| format!("{b:02x}")).collect()
        };
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let message_id = Ulid::new().to_string();

        let wtxn = self.db.begin_write()?;
        let seq = {
            let mut table = wtxn.open_table(MAIL)?;
            let lower = mail_key(recipient_pk, 0);
            let upper = mail_key(recipient_pk, u64::MAX);
            let seq = match table
                .range(lower.as_slice()..=upper.as_slice())?
                .next_back()
            {
                Some(Ok((k, _))) if k.value().len() >= 40 => {
                    u64::from_be_bytes(k.value()[32..40].try_into().unwrap()) + 1
                }
                _ => 0,
            };
            let entry = LogEntry {
                seq,
                nonce: [0u8; 24],
                ciphertext,
                message_id,
                author_id,
                timestamp_ms,
                deleted: false,
            };
            let key = mail_key(recipient_pk, seq);
            let value = postcard::to_allocvec(&entry).context("encode LogEntry")?;
            table.insert(key.as_slice(), value.as_slice())?;
            seq
        };
        wtxn.commit()?;
        Ok(seq)
    }

    /// Return all mailbox entries with `seq >= since_seq`.
    pub fn get_dms(&self, user_pk: &[u8; 32], since_seq: u64) -> Result<Vec<LogEntry>> {
        let lower = mail_key(user_pk, since_seq);
        let upper = mail_key(user_pk, u64::MAX);
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(MAIL)?;
        let mut entries = Vec::new();
        for item in table.range(lower.as_slice()..=upper.as_slice())? {
            let (_, v) = item?;
            entries.push(postcard::from_bytes::<LogEntry>(v.value()).context("decode LogEntry")?);
        }
        Ok(entries)
    }

    /// Look up the X25519 public key for a member identified by their Ed25519
    /// verifying key (`ed25519_pk`), scanning all stored community records.
    ///
    /// Returns `None` if the member is not found in any community on this node.
    pub fn x25519_pk_for_member(&self, ed25519_pk: &[u8; 32]) -> Result<Option<[u8; 32]>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(META)?;
        for item in table.iter()? {
            let (_, v) = item?;
            let meta: CommunityMeta =
                postcard::from_bytes(v.value()).context("decode CommunityMeta")?;
            for member in meta.members.values() {
                if member.public_key == *ed25519_pk {
                    return Ok(Some(member.x25519_public_key));
                }
            }
        }
        Ok(None)
    }

    // ── Community metadata ─────────────────────────────────────────────────

    /// Upsert community metadata.
    pub fn set_community_meta(&self, community_pk: &[u8; 32], meta: &CommunityMeta) -> Result<()> {
        let value = postcard::to_allocvec(meta).context("encode CommunityMeta")?;
        let wtxn = self.db.begin_write()?;
        {
            let mut table = wtxn.open_table(META)?;
            table.insert(community_pk.as_slice(), value.as_slice())?;
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Retrieve community metadata by community public key.
    pub fn get_community_meta(&self, community_pk: &[u8; 32]) -> Result<Option<CommunityMeta>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(META)?;
        match table.get(community_pk.as_slice())? {
            Some(v) => Ok(Some(
                postcard::from_bytes(v.value()).context("decode CommunityMeta")?,
            )),
            None => Ok(None),
        }
    }

    /// Remove all data associated with a community: metadata and channel logs.
    pub fn remove_community(&self, community_pk: &[u8; 32]) -> Result<()> {
        let wtxn = self.db.begin_write()?;
        {
            // Remove community metadata.
            let mut meta_table = wtxn.open_table(META)?;
            meta_table.remove(community_pk.as_slice())?;
        }
        {
            // Remove all channel log entries for this community.
            // Log keys are prefixed with community_pk(32), so scan the range.
            let mut log_table = wtxn.open_table(LOG)?;
            let lower = {
                let mut k = [0u8; 56];
                k[..32].copy_from_slice(community_pk);
                k
            };
            let upper = {
                let mut k = [0xffu8; 56];
                k[..32].copy_from_slice(community_pk);
                k
            };
            let keys_to_remove: Vec<Vec<u8>> = log_table
                .range(lower.as_slice()..=upper.as_slice())?
                .filter_map(|item| item.ok().map(|(k, _)| k.value().to_vec()))
                .collect();
            for key in keys_to_remove {
                log_table.remove(key.as_slice())?;
            }
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Return all unique user public keys that have a mailbox on this node.
    ///
    /// Used on startup to pre-populate the DHT with locally-held mailboxes
    /// so DM routing works immediately without waiting for new activity.
    pub fn all_mailbox_recipients(&self) -> Result<Vec<[u8; 32]>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(MAIL)?;
        let mut result: Vec<[u8; 32]> = Vec::new();
        let mut last_pk: Option<[u8; 32]> = None;
        for item in table.iter()? {
            let (k, _) = item?;
            if k.value().len() < 32 {
                continue;
            }
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&k.value()[..32]);
            // Keys are sorted, so equal PKs are consecutive — skip duplicates.
            if last_pk.as_ref() == Some(&pk) {
                continue;
            }
            last_pk = Some(pk);
            result.push(pk);
        }
        Ok(result)
    }

    /// Return all community public keys currently registered on this node.
    pub fn all_communities(&self) -> Result<Vec<[u8; 32]>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(META)?;
        let mut result = Vec::new();
        for item in table.iter()? {
            let (k, _) = item?;
            if k.value().len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(k.value());
                result.push(arr);
            }
        }
        Ok(result)
    }

    // ── Channel enumeration ───────────────────────────────────────────────

    /// Return all unique `(community_pk, channel_id)` pairs that have stored
    /// log entries.  Used by the background cache-retention task to iterate
    /// over every channel without needing an explicit channel registry.
    pub fn all_channel_ids(&self) -> Result<Vec<([u8; 32], Ulid)>> {
        let rtxn = self.db.begin_read()?;
        let table = rtxn.open_table(LOG)?;
        let mut result: Vec<([u8; 32], Ulid)> = Vec::new();
        let mut last_prefix: Option<([u8; 32], [u8; 16])> = None;
        for item in table.iter()? {
            let (k, _) = item?;
            if k.value().len() < 48 {
                continue;
            }
            let mut cpk = [0u8; 32];
            cpk.copy_from_slice(&k.value()[..32]);
            let mut chan = [0u8; 16];
            chan.copy_from_slice(&k.value()[32..48]);
            let pair = (cpk, chan);
            // Skip duplicates — keys are sorted so equal (cpk, chan) are consecutive.
            if last_prefix.as_ref() == Some(&pair) {
                continue;
            }
            last_prefix = Some(pair);
            let ulid = Ulid(u128::from_be_bytes(chan));
            result.push((cpk, ulid));
        }
        Ok(result)
    }

    // ── Retention policy ──────────────────────────────────────────────────

    /// Enforce a per-channel size limit: delete the oldest entries until the
    /// channel's total serialised size is ≤ `max_bytes`.
    ///
    /// This implements the retention policy: if a community
    /// exceeds the configured limit, the node evicts the oldest encrypted blobs.
    /// Because the data is encrypted, the node cannot read the content — it just
    /// deletes by insertion order.
    pub fn enforce_retention(
        &self,
        community_pk: &[u8; 32],
        channel_id: &Ulid,
        max_bytes: u64,
    ) -> Result<()> {
        let chan = ulid_be(channel_id);
        let lower = log_key(community_pk, &chan, 0);
        let upper = log_key(community_pk, &chan, u64::MAX);

        // First pass: measure total size and collect ordered keys.
        let rtxn = self.db.begin_read()?;
        let rtable = rtxn.open_table(LOG)?;
        let mut total: u64 = 0;
        let mut keys_asc: Vec<Vec<u8>> = Vec::new();
        for item in rtable.range(lower.as_slice()..=upper.as_slice())? {
            let (k, v) = item?;
            total += v.value().len() as u64;
            keys_asc.push(k.value().to_vec());
        }
        drop(rtable);
        drop(rtxn);

        if total <= max_bytes {
            return Ok(());
        }

        // Second pass: delete oldest entries until under the limit.
        let wtxn = self.db.begin_write()?;
        {
            let mut table = wtxn.open_table(LOG)?;
            for key in &keys_asc {
                if total <= max_bytes {
                    break;
                }
                if let Some(old) = table.remove(key.as_slice())? {
                    total = total.saturating_sub(old.value().len() as u64);
                }
            }
        }
        wtxn.commit()?;
        Ok(())
    }
}

impl CommunityMeta {
    pub fn community_pk(&self) -> [u8; 32] {
        self.cert.community_pk
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    fn open_store() -> (NodeStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = NodeStore::open(&dir.path().join("node.redb")).unwrap();
        (store, dir)
    }

    #[test]
    fn append_and_get_messages() {
        let (store, _dir) = open_store();
        let community_pk = [1u8; 32];
        let channel_id = Ulid::new();

        let seq0 = store
            .append_message(
                &community_pk,
                &channel_id,
                [0u8; 24],
                vec![1, 2],
                "id0".into(),
                "author".into(),
                1000,
            )
            .unwrap();
        let seq1 = store
            .append_message(
                &community_pk,
                &channel_id,
                [1u8; 24],
                vec![3, 4],
                "id1".into(),
                "author".into(),
                2000,
            )
            .unwrap();

        assert_eq!(seq0, 0);
        assert_eq!(seq1, 1);

        let all = store.get_messages(&community_pk, &channel_id, 0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 0);
        assert_eq!(all[1].seq, 1);

        let since1 = store.get_messages(&community_pk, &channel_id, 1).unwrap();
        assert_eq!(since1.len(), 1);
        assert_eq!(since1[0].seq, 1);
    }

    #[test]
    fn channels_are_independent() {
        let (store, _dir) = open_store();
        let community_pk = [2u8; 32];
        let ch_a = Ulid::new();
        let ch_b = Ulid::new();

        store
            .append_message(
                &community_pk,
                &ch_a,
                [0u8; 24],
                vec![],
                "a0".into(),
                "x".into(),
                0,
            )
            .unwrap();
        store
            .append_message(
                &community_pk,
                &ch_b,
                [0u8; 24],
                vec![],
                "b0".into(),
                "x".into(),
                0,
            )
            .unwrap();
        store
            .append_message(
                &community_pk,
                &ch_b,
                [1u8; 24],
                vec![],
                "b1".into(),
                "x".into(),
                1,
            )
            .unwrap();

        assert_eq!(
            store.get_messages(&community_pk, &ch_a, 0).unwrap().len(),
            1
        );
        assert_eq!(
            store.get_messages(&community_pk, &ch_b, 0).unwrap().len(),
            2
        );
    }

    #[test]
    fn dm_mailbox_roundtrip() {
        let (store, _dir) = open_store();
        use rand::RngCore;
        use x25519_dalek::{PublicKey, StaticSecret};

        let sender_sk = StaticSecret::random_from_rng(OsRng);
        let recipient_sk = StaticSecret::random_from_rng(OsRng);
        let recipient_pk_x = PublicKey::from(&recipient_sk);
        let envelope =
            crate::crypto::dm::DmEnvelope::seal(&sender_sk, &recipient_pk_x, b"hello").unwrap();

        let mut sender_pk = [0u8; 32];
        OsRng.fill_bytes(&mut sender_pk);
        let mut recipient_pk = [0u8; 32];
        OsRng.fill_bytes(&mut recipient_pk);

        let seq = store
            .append_dm(&recipient_pk, &sender_pk, &envelope)
            .unwrap();
        assert_eq!(seq, 0);

        let dms = store.get_dms(&recipient_pk, 0).unwrap();
        assert_eq!(dms.len(), 1);
        assert_eq!(dms[0].seq, 0);
    }

    #[test]
    fn community_meta_roundtrip() {
        let (store, _dir) = open_store();
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::generate(&mut OsRng);
        let node_pk = SigningKey::generate(&mut OsRng).verifying_key().to_bytes();
        let cert = crate::crypto::certificate::HostingCert::new(&sk, node_pk, u64::MAX);
        let community_pk = cert.community_pk;
        let meta = CommunityMeta {
            cert,
            manifest: None,
            channels: Vec::new(),
            channel_keys: HashMap::new(),
            members: HashMap::new(),
        };

        store.set_community_meta(&community_pk, &meta).unwrap();
        let loaded = store.get_community_meta(&community_pk).unwrap().unwrap();
        assert_eq!(loaded.community_pk(), community_pk);
    }

    #[test]
    fn all_channel_ids_returns_unique_pairs() {
        let (store, _dir) = open_store();
        let cpk_a = [10u8; 32];
        let cpk_b = [11u8; 32];
        let ch1 = Ulid::new();
        let ch2 = Ulid::new();

        // Two channels in community A, one in community B.
        for i in 0u8..3 {
            store
                .append_message(
                    &cpk_a,
                    &ch1,
                    [i; 24],
                    vec![],
                    format!("a1_{i}"),
                    "x".into(),
                    0,
                )
                .unwrap();
        }
        store
            .append_message(
                &cpk_a,
                &ch2,
                [0u8; 24],
                vec![],
                "a2_0".into(),
                "x".into(),
                0,
            )
            .unwrap();
        store
            .append_message(
                &cpk_b,
                &ch1,
                [0u8; 24],
                vec![],
                "b1_0".into(),
                "x".into(),
                0,
            )
            .unwrap();

        let ids = store.all_channel_ids().unwrap();
        // Expect exactly 3 unique (cpk, channel) pairs.
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn retention_deletes_oldest() {
        let (store, _dir) = open_store();
        let cpk = [5u8; 32];
        let ch = Ulid::new();

        // Insert 5 entries with 100-byte ciphertexts each.
        for i in 0u8..5 {
            store
                .append_message(
                    &cpk,
                    &ch,
                    [i; 24],
                    vec![i; 100],
                    format!("id{i}"),
                    "a".into(),
                    i as i64,
                )
                .unwrap();
        }

        // Retain at most 250 bytes (should keep the 3 newest, delete 2 oldest).
        store.enforce_retention(&cpk, &ch, 250).unwrap();

        let remaining = store.get_messages(&cpk, &ch, 0).unwrap();
        // Sizes: 100 × 3 = 300 bytes of ciphertext, but LogEntry serialisation overhead
        // may vary; check that some entries were deleted.
        assert!(
            remaining.len() < 5,
            "expected some entries deleted, got {}",
            remaining.len()
        );
    }
}
