# Push Event Catalog

Server-to-client push events are delivered via the `subscribe_events` / `unsubscribe_events`
JSON-RPC subscription (see `rpc_methods.md`).

Each notification has the form:
```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "subscription": "<subscription_id>",
    "result": {
      "type": "<event_type>",
      "data": { ... }
    }
  }
}
```

---

## Messages

### `message_new`
A new message was received in a channel the local node is subscribed to.

```json
{
  "type": "message_new",
  "data": {
    "message_id": "01HXYZ...",
    "channel_id": "01HABC...",
    "community_id": "01HDEF...",
    "author_id": "a1b2c3...",
    "timestamp": "2026-03-17T12:00:00Z"
  }
}
```
Frontend should call `message_get_history` (or use optimistic state) to fetch the full decrypted body.

### `message_edited`
An existing message was edited.

```json
{
  "type": "message_edited",
  "data": {
    "message_id": "...",
    "channel_id": "...",
    "community_id": "...",
    "author_id": "...",
    "timestamp": "..."
  }
}
```

### `message_deleted`
A message was deleted (tombstoned).

```json
{
  "type": "message_deleted",
  "data": {
    "message_id": "...",
    "channel_id": "...",
    "community_id": "..."
  }
}
```

---

## Members

### `member_joined`
A new member joined a community.

```json
{
  "type": "member_joined",
  "data": {
    "user_id": "...",
    "community_id": "...",
    "display_name": "Alice"
  }
}
```

### `member_left`
A member left (or was removed from) a community.

```json
{
  "type": "member_left",
  "data": {
    "user_id": "...",
    "community_id": "...",
    "display_name": "Alice"
  }
}
```

### `presence_changed`
A peer's presence status changed.

```json
{
  "type": "presence_changed",
  "data": {
    "user_id": "...",
    "status": "idle",
    "last_seen": "2026-03-17T12:05:00Z"
  }
}
```

---

## Channels

### `channel_created`
A new channel was created in a community.

```json
{
  "type": "channel_created",
  "data": {
    "channel_id": "...",
    "community_id": "...",
    "name": "general"
  }
}
```

### `channel_deleted`

```json
{
  "type": "channel_deleted",
  "data": {
    "channel_id": "...",
    "community_id": "...",
    "name": "old-channel"
  }
}
```

---

## Communities

### `community_manifest_updated`
The community manifest was updated (name, description, admins, seed nodes, etc.).

```json
{
  "type": "community_manifest_updated",
  "data": {
    "community_id": "...",
    "version": 5
  }
}
```

---

## Node

### `node_metrics_updated`
Emitted every 5 seconds with a fresh metrics snapshot.

```json
{
  "type": "node_metrics_updated",
  "data": {
    "connected_peers": 5,
    "stored_channels": 12,
    "disk_usage_mb": 47,
    "bandwidth_in_kbps": 120,
    "bandwidth_out_kbps": 80,
    "uptime_secs": 3600
  }
}
```

---

## Sync

### `sync_progress`
Reports CRDT sync progress for a channel (emitted during initial sync or after reconnect).

```json
{
  "type": "sync_progress",
  "data": {
    "channel_id": "...",
    "progress": 0.75
  }
}
```
`progress` is a float in `[0.0, 1.0]`. A value of `1.0` means sync is complete.
