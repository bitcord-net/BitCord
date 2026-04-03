# RPC Method Catalog

All methods are JSON-RPC 2.0 over WebSocket at `ws://127.0.0.1:7331`.

Request format:
```json
{"jsonrpc": "2.0", "method": "<method>", "params": <params>, "id": <id>}
```

---

## Identity

### `identity_get`
Returns the local node's identity information.

**Params:** none

**Result:**
```json
{
  "peer_id": "a1b2c3...",
  "display_name": "Alice",
  "status": "online",
  "public_key_hex": "deadbeef..."
}
```

### `identity_set_display_name`
**Params:** `{ "display_name": "Alice" }` (1–64 chars)
**Result:** `true`

### `identity_set_status`
**Params:** `{ "status": "online" | "idle" | "do_not_disturb" | "invisible" | "offline" }`
**Result:** `true`

### `identity_change_passphrase`
Re-encrypts the on-disk keystore with a new passphrase.
**Params:** `{ "old_passphrase": "...", "new_passphrase": "..." }`
**Result:** `true`

---

## Communities

### `community_create`
Creates a new community and publishes its manifest to the DHT.
**Params:** `{ "name": "My Community", "description": "...", "seed_nodes": ["/ip4/..."] }`
**Result:** `CommunityInfo`

### `community_join`
Joins a community via an invite link.
**Params:** `{ "invite": "<base64url invite payload>" }`
**Result:** `CommunityInfo`

### `community_leave`
**Params:** `"<community_id>"`
**Result:** `true`

### `community_list`
**Params:** none
**Result:** `CommunityInfo[]`

### `community_get`
**Params:** `"<community_id>"`
**Result:** `CommunityInfo`

### `community_update_manifest`
Admin-only. Updates community metadata.
**Params:** `{ "community_id": "...", "name"?: "...", "description"?: "...", "seed_nodes"?: [...] }`
**Result:** `true`

---

## Channels

### `channel_list`
**Params:** `"<community_id>"`
**Result:** `ChannelInfo[]`

### `channel_get`
**Params:** `"<channel_id>"`
**Result:** `ChannelInfo`

### `channel_create`
**Params:** `{ "community_id": "...", "name": "general", "kind": "text" | "announcement" | "voice" }`
**Result:** `ChannelInfo`

### `channel_delete`
Admin-only.
**Params:** `{ "community_id": "...", "channel_id": "..." }`
**Result:** `true`

### `channel_rotate_key`
Admin-only. Rotates the channel's symmetric key; removed members lose access.
**Params:** `{ "community_id": "...", "channel_id": "..." }`
**Result:** `true`

---

## Messages

### `message_send`
Encrypts and broadcasts a message to a channel.
**Params:**
```json
{
  "community_id": "...",
  "channel_id": "...",
  "body": "Hello!",
  "reply_to": "<message_id | null>"
}
```
**Result:** `MessageInfo`

### `message_edit`
**Params:** `{ "community_id": "...", "channel_id": "...", "message_id": "...", "body": "..." }`
**Result:** `true`

### `message_delete`
**Params:** `{ "community_id": "...", "channel_id": "...", "message_id": "..." }`
**Result:** `true`

### `message_get_history`
Returns paginated decrypted messages, newest-first.
**Params:** `{ "community_id": "...", "channel_id": "...", "before"?: "<message_id>", "limit"?: 50 }`
**Result:** `MessageInfo[]`

### `reaction_add`
**Params:** `{ "community_id": "...", "channel_id": "...", "message_id": "...", "emoji": "👍" }`
**Result:** `true`

### `reaction_remove`
**Params:** same as `reaction_add`
**Result:** `true`

---

## Members

### `member_list`
**Params:** `"<community_id>"`
**Result:** `MemberInfo[]`

### `member_kick`
Moderator+ only.
**Params:** `{ "community_id": "...", "user_id": "...", "reason"?: "..." }`
**Result:** `true`

### `member_ban`
Admin-only.
**Params:** `{ "community_id": "...", "user_id": "...", "reason"?: "..." }`
**Result:** `true`

### `member_update_role`
Admin-only.
**Params:** `{ "community_id": "...", "user_id": "...", "role": "admin" | "moderator" | "member" }`
**Result:** `true`

---

## Direct Messages

### `dm_send`
**Params:** `{ "peer_id": "...", "body": "..." }`
**Result:** `DmMessageInfo`

### `dm_get_history`
**Params:** `{ "peer_id": "...", "before"?: "<message_id>", "limit"?: 50 }`
**Result:** `DmMessageInfo[]`

---

## Node

### `node_get_metrics`
**Params:** none
**Result:**
```json
{
  "connected_peers": 5,
  "stored_channels": 12,
  "disk_usage_mb": 47,
  "bandwidth_in_kbps": 120,
  "bandwidth_out_kbps": 80,
  "uptime_secs": 3600
}
```

### `node_get_config`
**Params:** none
**Result:** `NodeConfigDto`

### `node_set_config`
All fields optional; only provided fields are updated.
**Params:**
```json
{
  "listen_addrs"?: [...],
  "seed_nodes"?: [...],
  "max_connections"?: 50,
  "storage_limit_mb"?: 512,
  "bandwidth_limit_kbps"?: null,
  "seed_priority"?: 0,
  "mdns_enabled"?: true,
  "log_level"?: "info"
}
```
**Result:** `true`

### `node_get_peers`
**Params:** none
**Result:** `PeerSummary[]`

---

## Push Event Subscription

### `subscribe_events` / `unsubscribe_events`
Subscribe to all server-to-client push events. See `push_events.md`.

**Params:** none

**Subscription notification method:** `"event"`

**websocat example:**
```sh
websocat ws://127.0.0.1:7331
> {"jsonrpc":"2.0","method":"subscribe_events","id":1}
< {"jsonrpc":"2.0","result":"<subscription_id>","id":1}
< {"jsonrpc":"2.0","method":"event","params":{"subscription":"<id>","result":{"type":"message_new","data":{...}}}}
```

---

## Error Codes

| Code   | Meaning                          |
|--------|----------------------------------|
| -32700 | Parse error                      |
| -32600 | Invalid request                  |
| -32601 | Method not found                 |
| -32602 | Invalid params                   |
| -32603 | Internal error                   |
| -32001 | Not found                        |
| -32099 | Not yet implemented              |
