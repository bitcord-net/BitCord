/**
 * BitCordClient — JSON-RPC 2.0 over WebSocket client.
 *
 * Features:
 * - Typed `call<T>()` for all RPC methods.
 * - `subscribe()` for server-push events via the `subscribe_events` subscription.
 * - Automatic reconnect with exponential backoff (max 30 s).
 * - Single WebSocket connection multiplexes requests and push events.
 */

import type {
  PushEventPayload,
  IdentityInfo,
  SetDisplayNameParams,
  SetStatusParams,
  ChangePassphraseParams,
  PresenceHeartbeatParams,
  CommunityInfo,
  CreateCommunityParams,
  JoinCommunityParams,
  UpdateManifestParams,
  ChannelInfo,
  CreateChannelParams,
  DeleteChannelParams,
  ReorderChannelsParams,
  RotateKeyParams,
  MessageInfo,
  SendMessageParams,
  EditMessageParams,
  DeleteMessageParams,
  GetHistoryParams,
  ReactionParams,
  MarkReadParams,
  MemberInfo,
  KickBanParams,
  UpdateRoleParams,
  DmMessageInfo,
  SendDmParams,
  GetDmHistoryParams,
  NodeMetricsSnapshot,
  NodeConfigDto,
  NodeLocalInfo,
  SetConfigParams,
  SetPreferredMailboxCommunityParams,
  PeerSummary,
} from "./rpc-types";

// ── Internal types ────────────────────────────────────────────────────────────

interface JsonRpcRequest {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
  id: number;
}

interface JsonRpcResponse {
  jsonrpc: "2.0";
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
  id: number | null;
}

interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: {
    subscription: string;
    result: unknown;
  };
}

export class RpcError extends Error {
  constructor(
    public readonly code: number,
    message: string,
    public readonly data?: unknown
  ) {
    super(message);
    this.name = "RpcError";
  }
}

type PushEventHandler = (event: PushEventPayload) => void;

// ── BitCordClient ─────────────────────────────────────────────────────────────

export class BitCordClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void }
  >();
  private pushHandlers = new Map<string, Set<PushEventHandler>>();
  private subscriptionId: string | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1_000; // ms; doubles on each failure, capped at 30 s
  private stopped = false;
  readonly url: string;

  constructor(url = "ws://127.0.0.1:7331") {
    this.url = url;
  }

  // ── Connection lifecycle ──────────────────────────────────────────────────

  connect(): void {
    if (this.stopped) return;
    if (
      this.ws &&
      (this.ws.readyState === WebSocket.OPEN ||
        this.ws.readyState === WebSocket.CONNECTING)
    ) {
      return;
    }
    this.ws = new WebSocket(this.url);

    this.ws.onopen = () => {
      this.reconnectDelay = 1_000; // reset on successful connect
      this._subscribeEvents();
    };

    this.ws.onmessage = (ev: MessageEvent) => {
      let msg: JsonRpcResponse | JsonRpcNotification;
      try {
        msg = JSON.parse(ev.data as string) as JsonRpcResponse | JsonRpcNotification;
      } catch {
        return;
      }

      // Notification (subscription push)
      if ("method" in msg && msg.method === "event") {
        const notif = msg as JsonRpcNotification;
        if (notif.params) {
          const subId = notif.params.subscription;
          const payload = notif.params.result as PushEventPayload;
          if (subId === this.subscriptionId) {
            this._dispatchPushEvent(payload);
          }
        }
        return;
      }

      // RPC response
      const resp = msg as JsonRpcResponse;
      if (resp.id == null) return;
      const pending = this.pending.get(resp.id);
      if (!pending) return;
      this.pending.delete(resp.id);

      if (resp.error) {
        pending.reject(
          new RpcError(resp.error.code, resp.error.message, resp.error.data)
        );
      } else {
        pending.resolve(resp.result);
      }
    };

    this.ws.onclose = () => {
      this._scheduleReconnect();
    };

    this.ws.onerror = () => {
      this.ws?.close();
    };
  }

  disconnect(): void {
    this.stopped = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
    }
    this._rejectAll(new Error("client disconnected"));
    this.ws?.close();
    this.ws = null;
  }

  get isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private _scheduleReconnect(): void {
    if (this.stopped) return;
    this.reconnectTimer = setTimeout(() => {
      this.connect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30_000);
  }

  /** Cancel any pending backoff timer and attempt to connect immediately. */
  forceReconnect(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.reconnectDelay = 1_000;
    this.stopped = false;
    this.ws?.close();
    this.ws = null;
    this.connect();
  }

  private _rejectAll(err: Error): void {
    for (const { reject } of this.pending.values()) {
      reject(err);
    }
    this.pending.clear();
  }

  // ── Core RPC ──────────────────────────────────────────────────────────────

  call<T>(method: string, params?: unknown): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (!this.isConnected) {
        reject(new Error("WebSocket not connected"));
        return;
      }
      const id = this.nextId++;
      const req: JsonRpcRequest = {
        jsonrpc: "2.0",
        method,
        params: params ?? null,
        id,
      };
      this.pending.set(id, {
        resolve: (v) => resolve(v as T),
        reject,
      });
      this.ws!.send(JSON.stringify(req));
    });
  }

  // ── Push event subscription ───────────────────────────────────────────────

  private async _subscribeEvents(): Promise<void> {
    try {
      this.subscriptionId = await this.call<string>("subscribe_events");
    } catch {
      // Will retry on reconnect
    }
  }

  subscribe<T extends PushEventPayload["type"]>(
    eventType: T,
    handler: (event: Extract<PushEventPayload, { type: T }>) => void
  ): () => void {
    if (!this.pushHandlers.has(eventType)) {
      this.pushHandlers.set(eventType, new Set());
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    this.pushHandlers.get(eventType)!.add(handler as any);
    return () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      this.pushHandlers.get(eventType)?.delete(handler as any);
    };
  }

  private _dispatchPushEvent(event: PushEventPayload): void {
    const handlers = this.pushHandlers.get(event.type);
    if (!handlers) return;
    for (const h of handlers) {
      h(event);
    }
  }

  // ── Typed method wrappers ─────────────────────────────────────────────────

  // Identity
  identityGet = () => this.call<IdentityInfo>("identity_get");
  identitySetDisplayName = (p: SetDisplayNameParams) =>
    this.call<boolean>("identity_set_display_name", p);
  identitySetStatus = (p: SetStatusParams) =>
    this.call<boolean>("identity_set_status", p);
  identityChangePassphrase = (p: ChangePassphraseParams) =>
    this.call<boolean>("identity_change_passphrase", p);

  // Communities
  communityCreate = (p: CreateCommunityParams) =>
    this.call<CommunityInfo>("community_create", p);
  communityJoin = (p: JoinCommunityParams) =>
    this.call<CommunityInfo>("community_join", p);
  communityLeave = (communityId: string) =>
    this.call<boolean>("community_leave", [communityId]);
  communityDelete = (communityId: string) =>
    this.call<boolean>("community_delete", [communityId]);
  communityList = () => this.call<CommunityInfo[]>("community_list");
  communityGet = (communityId: string) =>
    this.call<CommunityInfo>("community_get", [communityId]);
  communityUpdateManifest = (p: UpdateManifestParams) =>
    this.call<boolean>("community_update_manifest", p);
  communityGenerateInvite = (communityId: string) =>
    this.call<string>("community_generate_invite", [communityId]);

  // Channels
  channelList = (communityId: string) =>
    this.call<ChannelInfo[]>("channel_list", [communityId]);
  channelGet = (channelId: string) =>
    this.call<ChannelInfo>("channel_get", [channelId]);
  channelCreate = (p: CreateChannelParams) =>
    this.call<ChannelInfo>("channel_create", p);
  channelDelete = (p: DeleteChannelParams) =>
    this.call<boolean>("channel_delete", p);
  channelRotateKey = (p: RotateKeyParams) =>
    this.call<boolean>("channel_rotate_key", p);
  channelReorder = (p: ReorderChannelsParams) =>
    this.call<boolean>("channel_reorder", p);

  // Messages
  messageSend = (p: SendMessageParams) =>
    this.call<MessageInfo>("message_send", p);
  messageEdit = (p: EditMessageParams) =>
    this.call<boolean>("message_edit", p);
  messageDelete = (p: DeleteMessageParams) =>
    this.call<boolean>("message_delete", p);
  messageGetHistory = (p: GetHistoryParams) =>
    this.call<MessageInfo[]>("message_get_history", p);
  reactionAdd = (p: ReactionParams) =>
    this.call<boolean>("reaction_add", p);
  reactionRemove = (p: ReactionParams) =>
    this.call<boolean>("reaction_remove", p);
  markRead = (p: MarkReadParams) =>
    this.call<boolean>("mark_read", p);

  // Members
  memberList = (communityId: string) =>
    this.call<MemberInfo[]>("member_list", [communityId]);
  memberKick = (p: KickBanParams) => this.call<boolean>("member_kick", p);
  memberBan = (p: KickBanParams) => this.call<boolean>("member_ban", p);
  memberUpdateRole = (p: UpdateRoleParams) =>
    this.call<boolean>("member_update_role", p);

  // Direct Messages
  dmSend = (p: SendDmParams) => this.call<DmMessageInfo>("dm_send", p);
  dmGetHistory = (p: GetDmHistoryParams) =>
    this.call<DmMessageInfo[]>("dm_get_history", p);
  dmSetPreferredMailboxCommunity = (p: SetPreferredMailboxCommunityParams) =>
    this.call<string>("dm_set_preferred_mailbox_community", p);
  dmClearPreferredMailbox = () =>
    this.call<boolean>("dm_clear_preferred_mailbox");
  dmDiscard = (peerId: string, messageId: string) =>
    this.call<boolean>("dm_discard", { peer_id: peerId, message_id: messageId });
  dmPeerName = (peerId: string) =>
    this.call<string | null>("dm_peer_name", [peerId]);

  // Presence
  presenceHeartbeat = (p: PresenceHeartbeatParams) =>
    this.call<void>("presence_heartbeat", p);

  // Node
  nodeGetMetrics = () =>
    this.call<NodeMetricsSnapshot>("node_get_metrics");
  nodeGetConfig = () => this.call<NodeConfigDto>("node_get_config");
  nodeSetConfig = (p: SetConfigParams) =>
    this.call<boolean>("node_set_config", p);
  nodeGetPeers = () => this.call<PeerSummary[]>("node_get_peers");
  nodeGetLocalAddrs = () => this.call<NodeLocalInfo>("node_get_local_addrs");
}
