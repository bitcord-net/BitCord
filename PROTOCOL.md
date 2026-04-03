# BitCord Protocol Architecture

## Overview

BitCord uses two distinct transports:

- **QUIC** — peer-to-peer messaging between nodes (port 9042 by default)
- **JSON-RPC 2.0 over WebSocket** — local API between the GUI frontend and the embedded backend (port 7331)

These are never mixed: QUIC is only used for node-to-node communication; the frontend never speaks QUIC directly.

---

## TLS / Certificate Model

Each node generates a self-signed TLS certificate at startup. Connections are authenticated by comparing the SHA-256 fingerprint of the server's certificate DER bytes against a stored expected value:

| Connection type | Fingerprint used |
|---|---|
| Community seed node | Pinned fingerprint from the invite link, stored in `seed_fps`. Falls back to TOFU with a warning if no fingerprint is on record. |
| Global bootstrap seeds (`config.seed_nodes`) | `[0u8; 32]` — TOFU, accepts any certificate |
| DHT operations (kademlia lookups, mailbox propagation, DM routing) | `[0u8; 32]` — TOFU, the remote fingerprint is unknowable in advance |

The all-zeros fingerprint is the explicit TOFU sentinel: `FingerprintVerifier` accepts any certificate when `expected == [0u8; 32]`, otherwise it rejects on mismatch.

---

## QUIC Connection Protocol

Every QUIC connection between two nodes carries two independent stream patterns on the same connection:

### Bidirectional streams — request / response

The connecting client opens a new bidirectional stream for each request. The server reads the `ClientRequest`, performs the operation, and writes back a `ClientResponse`.

| Request | Purpose |
|---|---|
| `Authenticate` | Challenge-response: client signs a server nonce with its Ed25519 node key |
| `JoinCommunity` | Present a `HostingCert` to gain access to a community's channels |
| `SendMessage` | Append an encrypted message to a channel |
| `GetMessages` | Retrieve channel history since a given sequence number |
| `SendDm` | Deliver an encrypted DM envelope to a recipient's mailbox |
| `GetDms` | Retrieve queued DMs from the local mailbox |
| `FetchManifest` | Download a community's signed manifest + channel list |
| `FindNode` | DHT: return the K closest peers to a node ID |
| `StoreDhtRecord` | DHT: store a mailbox address record |
| `PushManifest` | Admin: push an updated community manifest to a hosting node |
| `PushHistory` | Admin: push channel history to a hosting node |
| `SendGossip` | Forward a gossip broadcast to this peer |

### Unidirectional streams — server push

The server opens unidirectional streams to deliver unsolicited events to authenticated clients. The client's `push_reader` task consumes these.

| Push event | Purpose |
|---|---|
| `NewMessage` | A new channel message arrived (real-time delivery) |
| `NewDm` | A DM was deposited in this node's mailbox |
| `Presence` | A peer's presence status changed |
| `GossipMessage` | A gossip broadcast forwarded from another peer |

Authentication is required before any push events are delivered. The server filters push events by the set of communities the client has joined in the current session.

---

## Deployment Modes

Three node modes are defined by the `NodeMode` enum in `bitcord_core::config`:

| Mode | Description |
|---|---|
| `GossipClient` | No QUIC server, no DHT, no mDNS. Pure gossip receiver. Connects outward to seed nodes only. |
| `Peer` *(default)* | Full peer: QUIC server + DHT + mDNS + gossip relay + user identity. The default desktop/mobile mode. |
| `HeadlessSeed` | Headless only. No user identity. Hosts communities, DM mailboxes, and DHT. Never emits `MemberJoined` or presence events. |

Set via `--mode <gossip-client|peer|headless-seed>` on the CLI, or `node_mode` in `node.toml` / via `node_set_config`.

---

### 1. HeadlessSeed — headless node

An always-on server with no GUI and no user identity. JSON-RPC API is not started.

```
Remote peer A ──QUIC──► [QUIC Server :9042]
Remote peer B ──QUIC──► [QUIC Server :9042]
                              │
                         NetworkHandle
                         gossip task
                              │
                    ┌─────────┴──────────┐
                    │     AppState        │
                    │  (communities,      │
                    │   channels,         │
                    │   NodeStore)        │
                    └────────────────────┘
```

**Inbound connections**: each accepted QUIC connection spawns a `ConnectionHandler` that:
1. Maintains a `ClientSession` (tracks authenticated peer + joined communities)
2. Runs a request loop accepting bidirectional streams
3. Runs a push relay task that filters and forwards `PushEvent`s from the broadcast channel

**Outbound connections**: the gossip task dials community seed nodes for manifest sync and gossip relay.

**Seed peer reachability**: `is_seed=true` on a `NetworkCommand::Dial` causes the gossip task to:
- Store the peer in the `seed_peers` map (enabling auto-reconnect on drop)
- Emit `SeedPeerConnected { community_id }` on successful join, which sets `reachable=true` on the community

---

### 2. Peer — GUI with embedded QUIC server

The default desktop/mobile configuration. The device acts as both a full QUIC node and a GUI client.

```
TypeScript / React frontend
        │
        │  WebSocket  JSON-RPC 2.0
        ▼
  API Server :7331  ◄──── subscribe_events (push subscription)
        │
        │  in-process
        ▼
     AppState
   ┌────┴─────────────────────────────────┐
   │  broadcaster (PushEvent channel)      │
   │  communities / channels / NodeStore   │
   └────┬─────────────────────────────────┘
        │
   NetworkHandle
   gossip task
        │
        │  outbound QUIC
        ▼
   Seed node / remote peers


Tauri Rust lib  ──QUIC NodeClient──►  QUIC Server :9042  (loopback)
  push relay task                            │
  (bridges push stream                  same AppState
   back to frontend)                         │
                                        NetworkHandle
```

**Two local connections on startup:**

1. **Tauri `NodeClient` → `127.0.0.1:9042`** (loopback QUIC)
   - Exists solely to receive the push stream from the embedded node
   - A background `push relay task` reads `NodePush` events and forwards them to:
     - The JSON-RPC `broadcaster` (so `subscribe_events` subscribers receive them)
     - Tauri native events (`app_handle.emit`) for lightweight frontend notifications
   - Not used for any request-response operations from the frontend

2. **TypeScript frontend → `ws://127.0.0.1:7331`** (JSON-RPC over WebSocket)
   - All frontend operations (send message, fetch history, community management, DMs, etc.)
   - `subscribe_events` call on connect to receive push notifications

**Gossip task outbound connections:**

The gossip task dials each community's seed nodes with `is_seed=true`. On a self-hosted device the seed address resolves to the device's own external IP (discovered via STUN). Since NAT hairpinning typically blocks loopback to one's own external IP, the dial fails; the gossip task detects the self-address and skips the reconnect loop. The community is marked reachable via the `self_seeded` check instead (comparing `public_addr` / `actual_listen_addrs` against the community's seed node list).

---

### 3. GossipClient — GUI without embedded QUIC server

A leaf-node client. No QUIC server is bound, so no other peer can dial it. It makes outbound connections to seed nodes only and is fully dependent on the seed as a message distribution hub. No loopback `NodeClient` is created.

```
TypeScript / React frontend
        │
        │  WebSocket  JSON-RPC 2.0
        ▼
  API Server :7331  ◄──── subscribe_events (push subscription)
        │
        │  in-process
        ▼
     AppState
   ┌────┴──────────────────────────────────┐
   │  broadcaster (PushEvent channel)       │
   │  communities / channels / NodeStore    │
   └────┬──────────────────────────────────┘
        │
   NetworkHandle
   gossip task
        │
        │  outbound QUIC
        ▼
   Seed node / remote peers
```

This client participates in gossip as a publisher and subscriber but not as a relay:

- **Receives**: the `push_reader` on each outbound connection receives `NodePush::GossipMessage` events pushed by the seed, which propagate through `process_swarm_events` → `broadcaster` → frontend.
- **Sends**: `NetworkCommand::Publish` calls `send_gossip()` on the outbound seed connection. The seed re-broadcasts to all of its inbound connections.
- **Cannot relay**: with no QUIC server, other peers cannot dial this node, it does not appear in DHT routing as a reachable address, and it never forwards gossip onwards.

The resulting topology is hub-and-spoke per community:

```
Client A ──outbound QUIC──► Seed node ◄──outbound QUIC── Client B
```

There is no loopback QUIC connection and no push relay task. Push events reach the frontend via a single path:

```
Remote peer
  └─► QUIC push stream
        └─► gossip task push_reader
              └─► NetworkEvent (gossip_evt_tx)
                    └─► process_swarm_events
                          └─► AppState.broadcaster.send(PushEvent)
                                └─► JSON-RPC subscribe_events notification
                                      └─► TypeScript frontend
```

The three Tauri commands `node_send_message`, `node_get_messages`, and `node_send_dm` are registered in `lib.rs` but are not called by the frontend in either GUI mode. All operations go through the JSON-RPC API.

---

## NetworkHandle / Gossip Task

The gossip task is a `tokio::select!` loop that manages all outbound QUIC connections and routes network events to `AppState`.

### Peer lifecycle

Two categories of outbound gossip connection exist. The `is_seed` flag on `NetworkCommand::Dial` / `PeerRegistration` determines which path a connection takes.

DHT bootstrap connections are **not** handled by the gossip task. They are managed exclusively by `DhtHandle` (see [DHT Layer](#dht-layer) below).

---

**Community seed nodes** (`is_seed = true`) — gossip relay infrastructure for a specific community. Dialed by `community_join` for each address in the invite's `seed_nodes` list, and by `community_create` when an external seed is configured.

```
NetworkCommand::Dial { is_seed: true, join_community: Some((pk, id)), ... }
  └─► NodeClient::connect(addr)
        └─► PeerRegistration { is_seed: true, join_community: Some((pk, id)), ... }
              ├─► push_reader spawned
              ├─► added to gossip peers map  (receives community gossip via Publish)
              ├─► stored in seed_peers map   (enables auto-reconnect on drop)
              └─► if this node IS the community admin:
                    HostingCert issued + JoinCommunity sent to seed
              └─► NetworkEvent::SeedPeerConnected { community_id }
                    └─► seed_connected_communities.insert(community_id)
                          └─► community.reachable = true
```

Seed nodes are **not** added to `AppState.connected_peers`. They are relay infrastructure tracked separately in the gossip task's `seed_peers` map.

---

**Community peers** (`is_seed = false`) — actual community members. Two discovery paths lead here:

*DHT discovery* (`DiscoverAndDial`): caller has already resolved peers via `DhtHandle::find_community_peers()`; community context is known at dial time.

```
DhtHandle::find_community_peers(community_pk)
  └─► NetworkCommand::DiscoverAndDial { peers: Vec<(node_pk, NodeAddr)>, community_pk, community_id }
        └─► for each peer:
              NodeClient::connect(addr)
                └─► PeerRegistration { is_seed: false,
                                       join_community: Some((pk, id)), ... }
                      ├─► push_reader spawned
                      ├─► added to gossip peers map
                      ├─► JoinCommunity sent (dummy cert or real HostingCert if admin)
                      └─► NetworkEvent::PeerConnected { peer_id, community_id }
                            └─► connected_peers[community_id].push(PeerSummary {
                                  is_admin: peer_id == hex(manifest.public_key), ...
                                })
                                FetchManifest sent to peer
```

*mDNS / LAN discovery*: community context is **unknown** at dial time. After connection, all joined communities are probed with `FetchManifest`. mDNS is active only in `Peer` mode; `GossipClient` mode skips both advertising and browsing.

```
mDNS ServiceResolved → NetworkCommand::Dial { is_seed: false, join_community: None, ... }
  └─► NodeClient::connect(addr)
        └─► PeerRegistration { join_community: None, ... }
              ├─► push_reader spawned
              ├─► added to gossip peers map
              └─► NetworkEvent::LanPeerConnected { peer_id }
                    └─► for each joined community:
                          FetchManifest { peer_id, community_id, community_pk }

                    When ManifestReceived fires from that peer:
                      └─► connected_peers[community_id].push(PeerSummary {
                            is_admin: peer_id == hex(manifest.public_key), ...
                          })
                          history sync triggered
```

`AppState.connected_peers` is a `HashMap<community_id, Vec<PeerSummary>>`. `PeerSummary.is_admin` is `true` when the peer's Ed25519 public key matches the community's public key. History fetches (`FetchChannelHistory`, `FetchManifest` after key rotation) prefer the admin peer.

On peer disconnect (detected via failed `send_gossip`):
- `PeerDisconnected(peer_id)` emitted → peer removed from all community lists in `connected_peers`
- If was a seed peer: `SeedPeerDisconnected` emitted → `reconnect_seed_loop` scheduled

Duplicate connections to the same peer are dropped; only the first `push_reader` is kept. `SeedPeerConnected` is still emitted for the new community in the duplicate case to handle multi-community seed peers.

### Gossip broadcast delivery

When a `NetworkCommand::Publish { topic, data }` is issued:

1. `send_gossip(topic, data)` is called on every connection in the gossip `peers` map. DHT-only bootstrap connections are **not** in this map and never receive community gossip.
2. The same message is also broadcast via `server_push_tx` (the inbound connection push channel), so clients that connected *to us* also receive it.

This ensures delivery to both dial-out and dial-in peers without a full mesh topology.

### Self-hosted seed detection

When a node's community seed address matches its own STUN-discovered external IP:
- The initial dial to the external IP fails (NAT hairpin)
- `own_addrs` (populated by `NetworkCommand::AddListenAddr` from STUN) is checked
- If the dial target is in `own_addrs`: reconnect loop is skipped, no `SeedPeerConnected` emitted
- `reachable` is computed statically via `self_seeded` in `community_list` / `community_get` by comparing the community's seed node list against `public_addr` and `actual_listen_addrs`

---

## Community Modes

BitCord communities have two operational modes determined by whether a seed node is configured:

### Seeded Communities (`seed_nodes` non-empty)

- **Message history**: persistent — the seed node stores all channel messages in its redb database; members who come online later can fetch history via `GetMessages`.
- **Reachability**: the `reachable` flag reflects connectivity to the seed (`SeedPeerConnected`/`SeedPeerDisconnected` events).
- **Topology**: hub-and-spoke through the seed, supplemented by direct P2P connections discovered via DHT.

### Seedless Communities (`seed_nodes` empty)

- **Message history**: ephemeral — messages exist only in the memory of currently connected peers; no central node stores history.
- **Reachability**: always `true` (no seed to be unreachable); the community is considered reachable as soon as it is created.
- **Topology**: fully peer-to-peer via DHT discovery; there is no central hub.

Seedless communities are IRC-like: suitable for low-latency presence-based communication where offline access is not needed.

The `seeded` flag on `CommunityInfo` (JSON-RPC) indicates which mode a community uses.

---

## DHT Community Peer Discovery

Both seeded and seedless communities use the DHT to help members find each other directly, reducing dependency on the seed as a single relay point.

### Record type: community peer

| Key | Value |
|---|---|
| `community_pk` (XOR target) | `[(node_pk, NodeAddr, announced_at)]` |

Up to K=20 entries per community, evicted by least-recently-seen when the bucket overflows. TTL = 1 hour.

### Lifecycle

| Event | Action |
|---|---|
| `community_create` | `dht.register_community_peer(community_pk).await` |
| `community_join` | `dht.register_community_peer(pk).await`; spawn `dht.find_community_peers(pk)` → `NetworkCommand::DiscoverAndDial` |
| `PeerConnected { peer_id, community_id }` | spawn `dht.find_community_peers(pk)` → `DiscoverAndDial` for all joined communities; add peer to `connected_peers[community_id]` |
| Hourly | `dht.register_community_peer(pk).await` for all communities; persist snapshot to redb |
| Node startup | Pre-populate in-memory DHT from redb |

### Wire protocol

Two new QUIC request types (no authentication required):

| Request | Response | Purpose |
|---|---|---|
| `StoreCommunityPeer { community_pk, node_pk, addr }` | `CommunityPeerAck` | Store a peer record in the remote node's DHT |
| `FindCommunityPeers { community_pk }` | `CommunityPeers(records)` | Retrieve known peers for a community |

### Kademlia lookup (`kademlia_find_community_peers`)

Two-phase iterative lookup targeting `community_pk`:

1. **Phase 1** — Standard iterative `FIND_NODE` walk (α=3, MAX_ROUNDS=8) to populate the K closest nodes in the routing table.
2. **Phase 2** — Query those K nodes with `FindCommunityPeers`; collect and deduplicate returned `CommunityPeerRecord`s. **Remote records take priority over locally cached ones**: the local on-disk cache is used only as a fallback for nodes not returned by any remote query. This prevents a stale on-disk address (e.g., from a previous session where NAT assigned a different ephemeral port) from shadowing a fresh address returned by the DHT seed.

TLS is TOFU (`[0u8; 32]` fingerprint) for all DHT operations since the remote node's identity is unknown in advance.

### Persistence (Fix #3)

Community peer records survive restarts:

- Written to `dht_community_peers` redb table: key = `community_pk(32) ++ node_pk(32)`, value = `postcard(CommunityPeerRecord)`.
- Loaded on startup and injected into the in-memory DHT before the gossip task starts.
- Expired records (older than 1 hour) pruned from disk every 10 minutes by the DHT expiry task.
- Full snapshot saved to disk hourly by the community presence re-announcement loop.

### Global bootstrap node

`bitcord.net:9042` is the hard-coded public bootstrap node (`BOOTSTRAP_NODES` in `config/bootstrap.rs`). It is dialed by `DhtHandle::bootstrap()` at startup with TOFU TLS. Its sole purpose is to seed the Kademlia routing table; it never receives community gossip and never appears in any peer list. Additional global bootstrap addresses can be added via `config.seed_nodes` and are dialed the same way. Bootstrap fires only in `Peer` and `HeadlessSeed` modes; `GossipClient` mode skips DHT entirely.

---

## DHT Layer

DHT operations are isolated in the `DhtHandle` type (`bitcord_core::dht`). The gossip task has no knowledge of DHT state; it only dials pre-resolved peers passed via `NetworkCommand::DiscoverAndDial`.

### Public API

```rust
impl DhtHandle {
    // Peer discovery (network RPCs)
    pub async fn find_mailbox_peers(&self, user_pk: [u8; 32]) -> Result<Vec<NodeAddr>>;
    pub async fn find_community_peers(&self, community_pk: [u8; 32]) -> Result<Vec<(NodePk, NodeAddr)>>;

    // Self-registration (network RPCs)
    pub async fn register_mailbox(&self, user_pk: [u8; 32]) -> Result<()>;
    pub async fn register_community_peer(&self, community_pk: [u8; 32]) -> Result<()>;
    pub fn update_self_addr(&self, addr: NodeAddr);

    // Local-only helpers (no network I/O)
    pub fn add_known_peer(&self, node_pk: [u8; 32], addr: NodeAddr);
    pub fn lookup_mailbox_local(&self, user_pk: [u8; 32]) -> Option<NodeAddr>;
    pub fn lookup_community_peers_local(&self, community_pk: [u8; 32]) -> Vec<(NodePk, NodeAddr)>;
    pub fn add_mailbox_record(&self, user_pk: [u8; 32], addr: NodeAddr);
    pub fn add_community_peer_record(&self, community_pk: [u8; 32], record: CommunityPeerRecord);

    // Startup
    pub async fn bootstrap(&self);  // dials bootstrap nodes, populates routing table
}
```

`DhtHandle` uses its own QUIC connections with TOFU TLS and never touches the gossip peer map. All callers receive `Option<Arc<DhtHandle>>`; it is `None` in `GossipClient` mode.

---

## JSON-RPC API

**Transport**: WebSocket, JSON-RPC 2.0
**Address**: `ws://127.0.0.1:7331`

### Method groups

| Group | Methods |
|---|---|
| Identity | `identity_get`, `identity_set_display_name`, `identity_set_status`, `identity_change_passphrase` |
| Community | `community_create`, `community_join`, `community_leave`, `community_delete`, `community_list`, `community_get`, `community_update_manifest`, `community_generate_invite` |
| Channel | `channel_list`, `channel_get`, `channel_create`, `channel_delete`, `channel_rotate_key`, `channel_reorder` |
| Message | `message_send`, `message_edit`, `message_delete`, `message_get_history`, `reaction_add`, `reaction_remove`, `mark_read` |
| DM | `dm_send`, `dm_get_history`, `dm_set_preferred_mailbox_community`, `dm_clear_preferred_mailbox` |
| Presence | `presence_heartbeat` |
| Node | `node_get_metrics`, `node_get_config`, `node_set_config`, `node_get_peers`, `node_get_local_addrs` |
| Subscription | `subscribe_events` |

### Push subscription

Call `subscribe_events` once on connect; the server returns a subscription ID. Subsequent push notifications arrive as JSON-RPC notifications with `method: "event"` and `params: { subscription, result: PushEventPayload }`.

| Push event type | Trigger |
|---|---|
| `message_new` | New channel message received or sent |
| `message_edit` | Message edited |
| `message_delete` | Message deleted |
| `reaction_update` | Reaction added or removed |
| `dm_new` | Incoming DM received |
| `seed_status_changed` | Seed peer connected or disconnected (`{ community_id, connected }`) |
| `presence_update` | Peer presence status changed |
| `community_joined` | This node successfully joined a community |
| `channel_history_received` | History sync completed for a channel |
| `manifest_update` | Community manifest updated by a remote peer |

`seed_status_changed` is the event the frontend uses to update the `reachable` flag on a community in real time, without requiring a full `community_list` refresh.
