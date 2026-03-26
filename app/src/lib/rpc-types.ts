// Auto-generated TypeScript interfaces matching Rust #[serde] structs in bitcord-core/src/api/types.rs
// Do not edit the struct shapes manually — update types.rs and regenerate.

// ── Identity ──────────────────────────────────────────────────────────────────

export type UserStatus = "online" | "idle" | "do_not_disturb" | "invisible" | "offline";

export interface IdentityInfo {
  peer_id: string;
  display_name: string | null;
  status: UserStatus;
  public_key_hex: string;
  public_addr: string | null;
  /** SHA-256 fingerprint of this node's TLS cert (64-char hex). Present only when the embedded server is running. */
  tls_fingerprint_hex: string | null;
}

export interface SetDisplayNameParams {
  display_name: string;
}

export interface SetStatusParams {
  status: UserStatus;
}

export interface ChangePassphraseParams {
  old_passphrase: string;
  new_passphrase: string;
}

// ── Communities ───────────────────────────────────────────────────────────────

export interface CommunityInfo {
  id: string;
  name: string;
  description: string;
  public_key_hex: string;
  admin_ids: string[];
  channel_ids: string[];
  seed_nodes: string[];
  version: number;
  created_at: string; // ISO 8601
  reachable: boolean;
}

export interface CreateCommunityParams {
  name: string;
  description: string;
  seed_nodes: string[];
  /** SHA-256 fingerprint of the seed node's TLS cert (64-char hex). Required when seed_nodes is non-empty. */
  seed_fingerprint_hex?: string | null;
  hosting_password?: string | null;
}

export interface JoinCommunityParams {
  /** Base64url-encoded invite payload. */
  invite: string;
  /** Password for private hosting nodes. Omit for open nodes. */
  hosting_password?: string;
}

export interface UpdateManifestParams {
  community_id: string;
  name?: string;
  description?: string;
  seed_nodes?: string[];
}

// ── Channels ──────────────────────────────────────────────────────────────────

export type ChannelKind = "text" | "announcement" | "voice";

export interface ChannelInfo {
  id: string;
  community_id: string;
  name: string;
  kind: ChannelKind;
  version: number;
  created_at: string;
}

export interface CreateChannelParams {
  community_id: string;
  name: string;
  kind: ChannelKind;
}

export interface DeleteChannelParams {
  community_id: string;
  channel_id: string;
}

export interface RotateKeyParams {
  community_id: string;
  channel_id: string;
}

// ── Messages ──────────────────────────────────────────────────────────────────

export interface ReactionInfo {
  emoji: string;
  user_ids: string[];
}

export interface MessageInfo {
  id: string;
  channel_id: string;
  community_id: string;
  author_id: string;
  timestamp: string;
  body: string;
  reply_to: string | null;
  edited_at: string | null;
  deleted: boolean;
  reactions: ReactionInfo[];
}

export interface SendMessageParams {
  community_id: string;
  channel_id: string;
  body: string;
  reply_to?: string | null;
}

export interface EditMessageParams {
  community_id: string;
  channel_id: string;
  message_id: string;
  body: string;
}

export interface DeleteMessageParams {
  community_id: string;
  channel_id: string;
  message_id: string;
}

export interface GetHistoryParams {
  community_id: string;
  channel_id: string;
  before?: string | null;
  limit?: number;
}

export interface ReactionParams {
  community_id: string;
  channel_id: string;
  message_id: string;
  emoji: string;
}

export interface MarkReadParams {
  community_id: string;
  channel_id: string;
  message_id: string;
}

// ── Members ───────────────────────────────────────────────────────────────────

export type RoleDto = "admin" | "moderator" | "member";

export interface MemberInfo {
  user_id: string;
  display_name: string;
  avatar_cid: string | null;
  roles: RoleDto[];
  joined_at: string;
  public_key_hex: string;
  status: UserStatus;
}

export interface KickBanParams {
  community_id: string;
  user_id: string;
  reason?: string | null;
}

export interface UpdateRoleParams {
  community_id: string;
  user_id: string;
  role: RoleDto;
}

// ── Direct Messages ───────────────────────────────────────────────────────────

export interface DmMessageInfo {
  id: string;
  peer_id: string;
  author_id: string;
  timestamp: string;
  body: string;
  reply_to: string | null;
  edited_at: string | null;
}

export interface SendDmParams {
  peer_id: string;
  body: string;
  reply_to?: string | null;
}

export interface GetDmHistoryParams {
  peer_id: string;
  before?: string | null;
  limit?: number;
}

// ── Node ──────────────────────────────────────────────────────────────────────

export interface NodeMetricsSnapshot {
  connected_peers: number;
  stored_channels: number;
  disk_usage_mb: number;
  bandwidth_in_kbps: number;
  bandwidth_out_kbps: number;
  uptime_secs: number;
}

export interface PeerSummary {
  peer_id: string;
  addresses: string[];
  latency_ms: number | null;
  relay_capable: boolean;
  reputation: number;
}

export interface NodeLocalInfo {
  node_address: string;
  listen_addrs: string[];
}

export interface NodeConfigDto {
  listen_addrs: string[];
  seed_nodes: string[];
  max_connections: number;
  storage_limit_mb: number;
  bandwidth_limit_kbps: number | null;
  is_seed_node: boolean;
  seed_priority: number;
  mdns_enabled: boolean;
  log_level: string;
  server_enabled: boolean;
  preferred_mailbox_node: string | null;
}

export interface SetConfigParams {
  listen_addrs?: string[];
  seed_nodes?: string[];
  max_connections?: number;
  storage_limit_mb?: number;
  bandwidth_limit_kbps?: number | null;
  is_seed_node?: boolean;
  seed_priority?: number;
  mdns_enabled?: boolean;
  log_level?: string;
  server_enabled?: boolean;
  /** `null` clears the preference; a string sets it. */
  preferred_mailbox_node?: string | null;
}

export interface SetPreferredMailboxCommunityParams {
  community_id: string;
}

// ── Push Events ───────────────────────────────────────────────────────────────

export interface MessageEventData {
  message_id: string;
  channel_id: string;
  community_id: string;
  author_id: string;
  author_name?: string;
  timestamp: string;
  body?: string | null;
}

export interface MessageDeletedData {
  message_id: string;
  channel_id: string;
  community_id: string;
}

export interface MemberEventData {
  user_id: string;
  community_id: string;
  display_name: string;
}

export interface PresenceChangedData {
  user_id: string;
  status: string;
  last_seen: string;
}

export interface ChannelEventData {
  channel_id: string;
  community_id: string;
  name: string;
}

export interface CommunityEventData {
  community_id: string;
  version: number;
  /** Human-readable reason when the deletion was caused by a join failure. */
  reason?: string;
}

export interface SyncProgressData {
  channel_id: string;
  /** Fraction in [0.0, 1.0]. */
  progress: number;
}

export interface DmNewData {
  message: DmMessageInfo;
}

export interface ChannelHistorySyncedData {
  channel_id: string;
  community_id: string;
}

export interface ReactionUpdatedData {
  message_id: string;
  channel_id: string;
  community_id: string;
  reactions: ReactionInfo[];
}

export interface PresenceHeartbeatParams {
  status: UserStatus;
}

export interface SeedStatusData {
  community_id: string;
  connected: boolean;
}

export interface MemberRoleUpdatedData {
  user_id: string;
  community_id: string;
  new_role: RoleDto;
}

export type PushEventPayload =
  | { type: "message_new"; data: MessageEventData }
  | { type: "message_edited"; data: MessageEventData }
  | { type: "message_deleted"; data: MessageDeletedData }
  | { type: "member_joined"; data: MemberEventData }
  | { type: "member_left"; data: MemberEventData }
  | { type: "presence_changed"; data: PresenceChangedData }
  | { type: "channel_created"; data: ChannelEventData }
  | { type: "channel_deleted"; data: ChannelEventData }
  | { type: "community_manifest_updated"; data: CommunityEventData }
  | { type: "community_deleted"; data: CommunityEventData }
  | { type: "node_metrics_updated"; data: NodeMetricsSnapshot }
  | { type: "sync_progress"; data: SyncProgressData }
  | { type: "dm_new"; data: DmNewData }
  | { type: "channel_history_synced"; data: ChannelHistorySyncedData }
  | { type: "reaction_updated"; data: ReactionUpdatedData }
  | { type: "seed_status_changed"; data: SeedStatusData }
  | { type: "member_role_updated"; data: MemberRoleUpdatedData };
