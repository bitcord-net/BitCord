/**
 * BitCordTestClient — Node.js JSON-RPC 2.0 over WebSocket client for integration tests.
 *
 * Adapted from app/src/lib/rpc-client.ts with:
 * - `ws` npm package instead of browser WebSocket
 * - `connectAndWait()` helper for test setup
 * - `waitForEvent()` helper for push event assertions
 */

import WebSocket from "ws";

// ── Types ─────────────────────────────────────────────────────────────────────

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

export interface PushEvent {
  type: string;
  [key: string]: unknown;
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

// ── Client ────────────────────────────────────────────────────────────────────

export class BitCordTestClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void }
  >();
  private pushHandlers = new Map<string, Set<(e: PushEvent) => void>>();
  private subscriptionId: string | null = null;
  readonly url: string;

  constructor(url: string) {
    this.url = url;
  }

  // ── Connection ────────────────────────────────────────────────────────────

  connect(): void {
    this.ws = new WebSocket(this.url);

    this.ws.on("open", () => {
      this._subscribeEvents().catch(() => {});
    });

    this.ws.on("message", (raw: Buffer | string) => {
      let msg: JsonRpcResponse | JsonRpcNotification;
      try {
        msg = JSON.parse(raw.toString()) as JsonRpcResponse | JsonRpcNotification;
      } catch {
        return;
      }

      // Push notification
      if ("method" in msg && msg.method === "event") {
        const notif = msg as JsonRpcNotification;
        if (notif.params) {
          if (notif.params.subscription === this.subscriptionId) {
            this._dispatchPushEvent(notif.params.result as PushEvent);
          }
        }
        return;
      }

      // RPC response
      const resp = msg as JsonRpcResponse;
      if (resp.id == null) return;
      const p = this.pending.get(resp.id);
      if (!p) return;
      this.pending.delete(resp.id);

      if (resp.error) {
        p.reject(new RpcError(resp.error.code, resp.error.message, resp.error.data));
      } else {
        p.resolve(resp.result);
      }
    });

    this.ws.on("close", () => {
      this._rejectAll(new Error("WebSocket closed"));
    });

    this.ws.on("error", (err: Error) => {
      this._rejectAll(err);
    });
  }

  /** Connect and wait until the WebSocket is open. */
  connectAndWait(timeoutMs = 5_000): Promise<void> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("WebSocket connect timeout")),
        timeoutMs
      );
      this.ws = new WebSocket(this.url);

      this.ws.on("open", () => {
        clearTimeout(timer);
        this._subscribeEvents().catch(() => {});
        resolve();
      });

      this.ws.on("message", (raw: Buffer | string) => {
        let msg: JsonRpcResponse | JsonRpcNotification;
        try {
          msg = JSON.parse(raw.toString()) as JsonRpcResponse | JsonRpcNotification;
        } catch {
          return;
        }
        if ("method" in msg && msg.method === "event") {
          const notif = msg as JsonRpcNotification;
          if (notif.params?.subscription === this.subscriptionId) {
            this._dispatchPushEvent(notif.params.result as PushEvent);
          }
          return;
        }
        const resp = msg as JsonRpcResponse;
        if (resp.id == null) return;
        const p = this.pending.get(resp.id);
        if (!p) return;
        this.pending.delete(resp.id);
        if (resp.error) {
          p.reject(new RpcError(resp.error.code, resp.error.message, resp.error.data));
        } else {
          p.resolve(resp.result);
        }
      });

      this.ws.on("close", () => {
        clearTimeout(timer);
        this._rejectAll(new Error("WebSocket closed"));
      });

      this.ws.on("error", (err: Error) => {
        clearTimeout(timer);
        reject(err);
      });
    });
  }

  close(): void {
    this._rejectAll(new Error("client closed"));
    this.ws?.close();
    this.ws = null;
  }

  get isOpen(): boolean {
    return this.ws?.readyState === 1; // OPEN
  }

  // ── RPC ───────────────────────────────────────────────────────────────────

  call<T = unknown>(method: string, params?: unknown): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (!this.isOpen) {
        reject(new Error("WebSocket not open"));
        return;
      }
      const id = this.nextId++;
      const req: JsonRpcRequest = { jsonrpc: "2.0", method, params: params ?? null, id };
      this.pending.set(id, {
        resolve: (v) => resolve(v as T),
        reject,
      });
      this.ws!.send(JSON.stringify(req));
    });
  }

  // ── Push events ───────────────────────────────────────────────────────────

  private async _subscribeEvents(): Promise<void> {
    try {
      this.subscriptionId = await this.call<string>("subscribe_events");
    } catch {
      // Retry is not needed in tests — subscriptions happen once on open.
    }
  }

  subscribe(eventType: string, handler: (e: PushEvent) => void): () => void {
    if (!this.pushHandlers.has(eventType)) {
      this.pushHandlers.set(eventType, new Set());
    }
    this.pushHandlers.get(eventType)!.add(handler);
    return () => this.pushHandlers.get(eventType)?.delete(handler);
  }

  /** Resolves with the first push event of `type` received within `timeoutMs`. */
  waitForEvent(eventType: string, timeoutMs = 10_000): Promise<PushEvent> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`Timeout waiting for push event: ${eventType}`)),
        timeoutMs
      );
      const unsub = this.subscribe(eventType, (event) => {
        clearTimeout(timer);
        unsub();
        resolve(event);
      });
    });
  }

  private _dispatchPushEvent(event: PushEvent): void {
    const handlers = this.pushHandlers.get(event.type);
    if (handlers) {
      for (const h of handlers) {
        h(event);
      }
    }
  }

  private _rejectAll(err: Error): void {
    for (const { reject } of this.pending.values()) {
      reject(err);
    }
    this.pending.clear();
  }

  // ── Typed helpers (subset needed by tests) ────────────────────────────────

  identityGet = () => this.call<Record<string, unknown>>("identity_get");

  communityCreate = (p: Record<string, unknown>) =>
    this.call<Record<string, unknown>>("community_create", p);

  communityJoin = (invite: string) =>
    this.call<Record<string, unknown>>("community_join", { invite });

  communityList = () =>
    this.call<Record<string, unknown>[]>("community_list");

  channelCreate = (p: Record<string, unknown>) =>
    this.call<Record<string, unknown>>("channel_create", p);

  channelList = (communityId: string) =>
    this.call<Record<string, unknown>[]>("channel_list", [communityId]);

  channelDelete = (p: Record<string, unknown>) =>
    this.call<boolean>("channel_delete", p);

  messageSend = (p: Record<string, unknown>) =>
    this.call<Record<string, unknown>>("message_send", p);

  messageDelete = (p: Record<string, unknown>) =>
    this.call<boolean>("message_delete", p);

  messageGetHistory = (p: Record<string, unknown>) =>
    this.call<Record<string, unknown>[]>("message_get_history", p);

  nodeGetLocalAddrs = () =>
    this.call<{ node_address: string; listen_addrs: string[] }>("node_get_local_addrs");

  nodeGetMetrics = () =>
    this.call<Record<string, unknown>>("node_get_metrics");
}
