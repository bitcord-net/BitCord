# BitCord Testing

BitCord has two tiers of automated integration tests: in-process Rust tests that drive the API and QUIC node layers directly, and TypeScript process-level tests that spawn real `bitcord-node` binaries.

---

## Quick start

```bash
# Tier 1 — Rust tests (fastest, no binary required)
cargo test --workspace

# Tier 2 — TypeScript process tests (requires built binary)
cargo build -p bitcord-node
cd tests/integration && npm install && npm test
```

---

## Architecture overview

The backend is split across three crates:

- **`bitcord-core`** — library crate containing the full P2P stack, QUIC node server/client, in-memory state, and JSON-RPC 2.0 API server.
- **`bitcord-node`** — headless binary that runs the backend without a UI. Used by the TypeScript tests.
- **`bitcord-tauri`** — desktop app; embeds the same core as `bitcord-node` with a Tauri frontend.

The JSON-RPC API (`rpc_server.rs`) is the primary integration surface. It exposes all application commands over a WebSocket connection (default `127.0.0.1:7331`). Key methods exercised by tests:

| Method | Description |
|---|---|
| `identity_get` | Returns peer ID, display name, public key |
| `community_create` | Creates a new community |
| `community_join` | Joins via base64url invite payload |
| `community_list` | Lists all communities |
| `channel_create` | Creates a Text or Announcement channel |
| `channel_list` | Lists channels for a community |
| `channel_delete` | Removes a channel |
| `message_send` | Encrypts and publishes a message via GossipSub |
| `message_delete` | Tombstones a message (author only) |
| `message_get_history` | Returns decrypted message history |
| `member_update_role` | Promotes/demotes a community member |
| `member_kick` | Removes a member from a community |
| `member_ban` | Bans a member from a community |
| `node_get_local_addrs` | Returns the node's listen addresses |
| `node_get_metrics` | Returns runtime metrics snapshot |
| `node_get_config` | Returns current node config |
| `node_set_config` | Updates node config fields |
| `subscribe_events` | Subscribes to push events on this WebSocket |

### Invite link format

`community_join` takes a base64url-encoded JSON payload:

```json
{
  "community_id": "<ULID string>",
  "name": "Community Name",
  "description": "...",
  "seed_nodes": ["192.168.1.5:7332"],
  "public_key_hex": "aabbcc..."
}
```

Seed node addresses are plain `host:port` strings taken from `node_get_local_addrs`.

### Push events

After calling `subscribe_events`, the server pushes notifications on the same WebSocket:

```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "subscription": "<subscription-id>",
    "result": { "type": "message_new", ... }
  }
}
```

Event types include: `message_new`, `message_deleted`, `message_edited`, `channel_created`, `channel_deleted`, `community_manifest_updated`, `member_joined`, `dm_new`, `channel_history_synced`.

---

## Tier 1 — Rust tests

All Rust tests live in `crates/bitcord-core/tests/`. They run in-process with no child processes or external services.

### Test files

| File | Layer | What it tests |
|---|---|---|
| `api_integration.rs` | JSON-RPC API | `identity_get`, `node_get_metrics`, `node_get_config`, `node_set_config`, `message_delete` (tombstone + authorship check) |
| `roles_integration.rs` | JSON-RPC API | Role promotion/demotion, kick, ban, announcement-channel post restrictions — all permission paths for Admin/Moderator/Member |
| `node_integration.rs` | QUIC node (NodeServer/NodeClient) | Auth roundtrip, message send/get, monotonic seqs, cert validation, DM mailbox, push notifications, unauthenticated/malformed request rejection |
| `multi_node_e2e.rs` | QUIC node (two instances) | Independent stores per node, shared community cert on both nodes, cross-node cert mismatch rejection, concurrent clients |

### Test patterns

**API-layer tests** (`api_integration.rs`, `roles_integration.rs`) use a minimal `AppState` seeded directly (no swarm, no disk I/O). They start an `ApiServer` on port 0, connect a `jsonrpsee` WebSocket client, and call RPC methods:

```rust
let node = make_test_node(&tmp);
// Seed state directly for tests that need a community:
node.state.communities.write().await.insert(cid, signed_manifest);
node.state.channel_keys.write().await.insert(chid, key_bytes);

let handle = ApiServer::start("127.0.0.1:0".parse()?, node.state).await?;
let client = WsClientBuilder::default().build(handle.local_addr()).await?;
let result: Value = client.request("some_method", params).await?;
```

**QUIC-layer tests** (`node_integration.rs`, `multi_node_e2e.rs`) bind real `NodeServer` instances on loopback with port 0, connect `NodeClient`s with TOFU TLS, and exercise the wire protocol:

```rust
let server = NodeServer::bind("127.0.0.1:0", &tls_cert, services).await?;
let (client, _, push_rx) = NodeClient::connect(server.local_addr(), fingerprint, identity).await?;
client.join_community(cert, None, None).await?;
let seq = client.send_message(community_pk, channel_id, nonce, ciphertext).await?;
```

### Running Rust tests

```bash
# All crate tests
cargo test --package bitcord-core

# Specific test file
cargo test --package bitcord-core --test api_integration
cargo test --package bitcord-core --test roles_integration
cargo test --package bitcord-core --test node_integration
cargo test --package bitcord-core --test multi_node_e2e

# Filter by test name
cargo test --package bitcord-core message_delete
```

---

## Tier 2 — TypeScript process tests

Lives in `tests/integration/`. Spawns real `bitcord-node` processes over localhost, connects via the JSON-RPC WebSocket API, and asserts observable behaviour including cross-node propagation.

### Directory layout

```
tests/integration/
  package.json                          vitest + ws + @types/ws
  tsconfig.json
  vitest.config.ts                      60 s timeout, sequential execution
  helpers/
    BitCordTestClient.ts                ws-based JSON-RPC 2.0 client
    NodeProcess.ts                      spawns bitcord-node, waits for API readiness
    TestLogger.ts                       structured JSON report writer
  scenarios/
    01-community-lifecycle.test.ts      community create → join → channel sync → delete
    02-message-sync.test.ts             message send/receive + delete propagation
    03-channel-management.test.ts       channel CRUD + metrics (single node)
  test-results/                         gitignored; JSON reports written here
```

### Port allocation

Sequential execution is enforced by the vitest config to prevent port conflicts:

| Scenario | Node A API | Node B API |
|---|---|---|
| 01 community lifecycle | 7401 | 7411 |
| 02 message sync | 7421 | 7431 |
| 03 channel management | 7441 | — |

The QUIC port is set to `0` in each node's generated config so the OS picks a free port automatically.

### Building and running

```bash
# 1. Build the node binary (debug is fine)
cargo build -p bitcord-node

# 2. Install Node.js dependencies (one-time)
cd tests/integration && npm install

# 3. Run all scenarios
npm test

# Or point at a release binary
BITCORD_NODE_BIN=../../target/release/bitcord-node npm test
```

`BITCORD_NODE_BIN` defaults to `../../target/debug/bitcord-node` relative to `tests/integration/`.

### How NodeProcess works

`NodeProcess` writes a minimal `config.toml` to a temp directory, then spawns the binary with:

```
BITCORD_PASSPHRASE=""     — skips passphrase prompt
BITCORD_TEST_MODE=1       — switches tracing to JSON format (one object per line)
--api-port <N>            — sets the JSON-RPC listen port
--config <path>           — points at the generated config.toml
```

It polls the WebSocket port every 300 ms until the API responds (up to 15 s). On `stop()`, SIGTERM is sent and the temp directory is cleaned up.

### How BitCordTestClient works

A thin wrapper around the `ws` npm package that speaks raw JSON-RPC 2.0 — the same wire format as the browser client in `app/src/lib/rpc-client.ts`. Key methods:

```typescript
await client.connectAndWait(5_000);           // open + subscribe to push events
await client.messageSend({ ... });            // typed helpers for common methods
await client.messageDelete({ ... });
await client.messageGetHistory({ ... });
await client.waitForEvent("message_new", 10_000); // resolves on next matching push event
await client.call("any_method", params);      // generic escape hatch
```

### How TestLogger works

Each scenario wraps steps in `logger.step(name, fn)`. Steps are timed and capture pass/fail. At the end of the suite, `logger.writeReport()` writes a JSON file to `test-results/`:

```json
{
  "suite": "message-sync",
  "total": 6, "passed": 6, "failed": 0,
  "steps": [
    { "step": "message_send", "status": "pass", "duration_ms": 12 },
    { "step": "message_delete_sync_b", "status": "pass", "duration_ms": 843 }
  ],
  "node_logs": [
    { "timestamp": "...", "level": "INFO", "message": "tombstoned message", "source": "node-a" }
  ]
}
```

Node logs (captured from the binary's JSON output) are included alongside test steps so failures can be diagnosed from the report alone.

---

## Docker variant

For fully isolated runs or reproducing CI failures locally:

```bash
docker compose -f docker-compose.test.yml up --abort-on-container-exit
```

This starts `node-a` (API 7401) and `node-b` (API 7411) as separate containers, waits for their health checks, then runs the test suite in a Node.js container. `BITCORD_DOCKER_MODE=1` is set automatically — `NodeProcess` skips spawning and instead connects to the pre-running containers via `NODE_A_URL`/`NODE_B_URL`.

---

## CI

Three jobs run on every push and PR:

| Job | What it runs |
|---|---|
| `rust` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` |
| `integration-ts` | `cargo build --release -p bitcord-node`, then `npm test` in `tests/integration/` |
| `frontend` | `npm run lint`, `npm run build` in `app/` |

The Rust job covers all four test files in `crates/bitcord-core/tests/`. The TypeScript job runs all three scenarios against the release binary. Jobs run in parallel; `integration-ts` does not depend on `rust`.

---

## Adding new tests

### New RPC method test (Rust)

Add a `#[tokio::test]` to `api_integration.rs`. Seed the required state directly into `AppState` (communities, channel_keys, message_log), start `ApiServer` on port 0, connect with `WsClientBuilder`, and call via `ObjectParams`. See `message_delete_tombstones_own_message` for the full pattern.

### New permission test (Rust)

Add a test to `roles_integration.rs`. Use the existing `seed_community` / `seed_member` / `seed_announcement_channel` helpers, call `start(state)` to get a client, and assert that the RPC either succeeds or returns a specific error string.

### New TypeScript scenario

Create `tests/integration/scenarios/NN-description.test.ts`. Pick unused ports from the allocation table above. Wrap all assertions with `logger.step(name, fn)` and call `logger.writeReport()` in `afterAll`. Use `pollUntil` from `TestLogger.ts` for anything that requires cross-node propagation.

### New RPC method

When adding a method to `rpc_server.rs`:
1. Add it to the method table in this document
2. Add a Rust test in `api_integration.rs` covering the happy path and at least one error case
3. Add a typed helper to `BitCordTestClient.ts`
4. Cover the end-to-end path in a TypeScript scenario if the method involves network propagation
