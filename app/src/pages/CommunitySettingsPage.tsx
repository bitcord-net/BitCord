import React, { useState, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, RotateCw, AlertTriangle, Server, Mailbox } from "lucide-react";
import { rpcClient } from "../hooks/useRpc";
import { toast } from "../store/toast";
import { useCommunitiesStore } from "../store/communities";
import { useIdentityStore } from "../store/identity";
import { useSettingsStore, type NotificationLevel } from "../store/settings";
import type { CommunityInfo } from "../lib/rpc-types";

type SettingsTab = "general" | "seed_nodes" | "admins" | "channels" | "notifications" | "dm" | "danger";

const ADMIN_TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "General" },
  { key: "seed_nodes", label: "Seed Node" },
  { key: "admins", label: "Admins" },
  { key: "channels", label: "Channels" },
  { key: "notifications", label: "Notifications" },
  { key: "dm", label: "Direct Messages" },
  { key: "danger", label: "Danger Zone" },
];

const MEMBER_TABS: { key: SettingsTab; label: string }[] = [
  { key: "notifications", label: "Notifications" },
  { key: "dm", label: "Direct Messages" },
];

const inputStyle: React.CSSProperties = {
  width: "100%",
  padding: "0.5rem 0.75rem",
  background: "var(--color-bc-surface-3)",
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: "4px",
  color: "var(--color-bc-text)",
  fontSize: "0.9375rem",
  outline: "none",
  boxSizing: "border-box",
};

const labelTextStyle: React.CSSProperties = {
  display: "block",
  fontSize: "0.75rem",
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: "0.05em",
  color: "var(--color-bc-muted)",
  marginBottom: "0.375rem",
};

// ── General settings tab ──────────────────────────────────────────────────────

function GeneralTab({ community }: { community: CommunityInfo }) {
  const { updateCommunity } = useCommunitiesStore();
  const [name, setName] = useState(community.name);
  const [description, setDescription] = useState(community.description);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      await rpcClient.communityUpdateManifest({
        community_id: community.id,
        name: name.trim(),
        description: description.trim(),
      });
      updateCommunity({ ...community, name: name.trim(), description: description.trim() });
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch {
      setError("Community renaming is not yet available.");
    } finally {
      setSaving(false);
    }
  };

  return (
    <form onSubmit={handleSave}>
      <label style={{ display: "block", marginBottom: "1.25rem" }}>
        <span style={labelTextStyle}>Community Name</span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={100}
          required
          style={inputStyle}
        />
      </label>
      <label style={{ display: "block", marginBottom: "1.25rem" }}>
        <span style={labelTextStyle}>Description</span>
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={4}
          maxLength={500}
          style={{ ...inputStyle, resize: "vertical" }}
        />
      </label>
      <div style={{ marginBottom: "1rem" }}>
        <span style={labelTextStyle}>Community ID</span>
        <div style={{ fontFamily: "monospace", fontSize: "0.875rem", color: "var(--color-bc-muted)", padding: "0.5rem 0" }}>
          {community.id}
        </div>
      </div>
      {error && <p style={{ color: "var(--color-bc-danger)", margin: "0 0 1rem", fontSize: "0.875rem" }}>{error}</p>}
      <button
        type="submit"
        disabled={saving || !community.reachable}
        style={{
          padding: "0.5rem 1.25rem",
          background: saved ? "var(--color-bc-success)" : "var(--color-bc-accent)",
          border: "none",
          borderRadius: "4px",
          color: "#fff",
          cursor: saving ? "wait" : "pointer",
          fontWeight: 600,
          fontSize: "0.9375rem",
        }}
      >
        {saving ? "Saving…" : saved ? "Saved!" : "Save Changes"}
      </button>
    </form>
  );
}

// ── Seed node tab ─────────────────────────────────────────────────────────────

function SeedNodesTab({ community }: { community: CommunityInfo }) {
  const { updateCommunity } = useCommunitiesStore();
  const [seedNode, setSeedNode] = useState(community.seed_nodes[0] ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      const seed_nodes = seedNode.trim() ? [seedNode.trim()] : [];
      await rpcClient.communityUpdateManifest({ community_id: community.id, seed_nodes });
      updateCommunity({ ...community, seed_nodes });

      toast("Seed node updated.");
    } catch {
      setError("Failed to update seed node.");
    } finally {
      setSaving(false);
    }
  };

  const isLocalOnly = !seedNode.trim();

  return (
    <div>
      <div
        style={{
          border: `1px solid ${isLocalOnly ? "var(--color-bc-accent)" : "rgba(255,255,255,0.08)"}`,
          borderRadius: "8px",
          padding: "1rem 1.25rem",
          marginBottom: "1.75rem",
          background: isLocalOnly ? "rgba(88,101,242,0.07)" : "var(--color-bc-surface-3)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: "0.625rem", marginBottom: "0.5rem" }}>
          <Server size={18} style={{ color: isLocalOnly ? "var(--color-bc-accent)" : "var(--color-bc-muted)", flexShrink: 0 }} />
          <span style={{ fontWeight: 600, fontSize: "0.9375rem", color: "var(--color-bc-text)" }}>
            Seed Node
          </span>
        </div>
        <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", margin: "0 0 1rem" }}>
          {isLocalOnly
            ? "This community is local-only — it is only reachable while this device is online. Set a seed node address (an always-on bitcord-node) so members can stay in sync even when you're offline."
            : "The always-on node that hosts this community. Members sync through it when your device is offline."}
        </p>
        <div style={{ display: "flex", gap: "0.5rem" }}>
          <input
            type="text"
            value={seedNode}
            onChange={(e) => setSeedNode(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void handleSave();
              }
            }}
            placeholder="host:port  (e.g. 203.0.113.5:9042)"
            style={{ ...inputStyle, flex: 1 }}
            disabled={saving}
          />
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={saving}
            style={{
              padding: "0.5rem 1rem",
              background: "var(--color-bc-accent)",
              border: "none",
              borderRadius: "4px",
              color: "#fff",
              cursor: saving ? "not-allowed" : "pointer",
              fontWeight: 600,
              fontSize: "0.875rem",
              opacity: saving ? 0.6 : 1,
              whiteSpace: "nowrap",
            }}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
        {error && (
          <p style={{ color: "var(--color-bc-danger)", margin: "0.5rem 0 0", fontSize: "0.875rem" }}>
            {error}
          </p>
        )}
      </div>
    </div>
  );
}

// ── Admins tab ────────────────────────────────────────────────────────────────

function AdminsTab({ community }: { community: CommunityInfo }) {
  return (
    <div>
      <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", marginTop: 0, marginBottom: "1rem" }}>
        Admin peer IDs for this community.
      </p>
      {community.admin_ids.map((id) => (
        <div key={id} style={{ padding: "0.5rem 0.75rem", background: "var(--color-bc-surface-3)", borderRadius: "4px", marginBottom: "0.25rem", fontFamily: "monospace", fontSize: "0.8125rem", color: "var(--color-bc-text)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {id}
        </div>
      ))}
    </div>
  );
}

// ── Channels (key rotation) tab ───────────────────────────────────────────────

function ChannelsTab({ community }: { community: CommunityInfo }) {
  const { channels } = useCommunitiesStore();
  const channelList = channels[community.id] ?? [];
  const [rotating, setRotating] = useState<string | null>(null);
  const [rotated, setRotated] = useState<string | null>(null);

  const handleRotate = async (channelId: string) => {
    setRotating(channelId);
    try {
      await rpcClient.channelRotateKey({ community_id: community.id, channel_id: channelId });
      setRotated(channelId);
      setTimeout(() => setRotated(null), 2000);
    } catch {
      toast("Failed to rotate channel key.", "error");
    } finally {
      setRotating(null);
    }
  };

  return (
    <div>
      <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", marginTop: 0, marginBottom: "1rem" }}>
        Rotate channel encryption keys. All current members will receive the new key.
      </p>
      {channelList.filter((ch) => ch.kind === "text" || ch.kind === "announcement").map((ch) => (
        <div key={ch.id} style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0.625rem 0.75rem", background: "var(--color-bc-surface-3)", borderRadius: "4px", marginBottom: "0.25rem" }}>
          <span style={{ color: "var(--color-bc-text)", fontWeight: 500 }}>#{ch.name}</span>
          <button
            onClick={() => void handleRotate(ch.id)}
            disabled={rotating === ch.id || !community.reachable}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.375rem",
              padding: "0.375rem 0.75rem",
              background: rotated === ch.id ? "var(--color-bc-success)" : "var(--color-bc-surface-1)",
              border: "none",
              borderRadius: "4px",
              color: "#fff",
              cursor: rotating === ch.id ? "wait" : !community.reachable ? "not-allowed" : "pointer",
              opacity: !community.reachable ? 0.4 : 1,
              fontSize: "0.875rem",
              fontWeight: 500,
            }}
          >
            <RotateCw size={14} />
            {rotating === ch.id ? "Rotating…" : rotated === ch.id ? "Rotated!" : "Rotate Key"}
          </button>
        </div>
      ))}
      {channelList.length === 0 && (
        <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem" }}>No channels.</p>
      )}
    </div>
  );
}

// ── Notifications tab ─────────────────────────────────────────────────────────

const LEVEL_LABELS: Record<NotificationLevel, string> = {
  all: "All Messages",
  mentions: "Mentions Only",
  none: "None",
};

function NotificationLevelPicker({
  value,
  onChange,
}: {
  value: NotificationLevel;
  onChange: (level: NotificationLevel) => void;
}) {
  return (
    <div style={{ display: "flex", gap: "0.5rem" }}>
      {(["all", "mentions", "none"] as NotificationLevel[]).map((l) => (
        <button
          key={l}
          onClick={() => onChange(l)}
          style={{
            flex: 1,
            padding: "0.5rem 0.25rem",
            borderRadius: "6px",
            cursor: "pointer",
            border: `2px solid ${value === l ? "var(--color-bc-accent)" : "rgba(255,255,255,0.08)"}`,
            background: "var(--color-bc-surface-3)",
            color: value === l ? "var(--color-bc-text)" : "var(--color-bc-muted)",
            fontWeight: value === l ? 600 : 400,
            fontSize: "0.875rem",
          }}
        >
          {LEVEL_LABELS[l]}
        </button>
      ))}
    </div>
  );
}

function NotificationsTab({ community }: { community: CommunityInfo }) {
  const { channels } = useCommunitiesStore();
  const {
    defaultNotificationLevel,
    notificationOverrides,
    setNotificationOverride,
    setChannelNotificationOverride,
  } = useSettingsStore();

  const channelList = channels[community.id] ?? [];
  const override = notificationOverrides.find((o) => o.communityId === community.id);
  const communityLevel: NotificationLevel = override?.level ?? defaultNotificationLevel;

  return (
    <div>
      <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", marginTop: 0, marginBottom: "1.25rem" }}>
        Override notification settings for this community. Channel-level settings take priority over community-level.
      </p>

      <span style={labelTextStyle}>Community Notifications</span>
      <div style={{ marginBottom: "1.5rem" }}>
        <NotificationLevelPicker
          value={communityLevel}
          onChange={(level) => setNotificationOverride(community.id, level)}
        />
      </div>

      {channelList.filter((ch) => ch.kind === "text" || ch.kind === "announcement").length > 0 && (
        <>
          <span style={labelTextStyle}>Per-Channel Overrides</span>
          {channelList
            .filter((ch) => ch.kind === "text" || ch.kind === "announcement")
            .map((ch) => {
              const chLevel: NotificationLevel = override?.channelOverrides[ch.id] ?? communityLevel;
              return (
                <div key={ch.id} style={{ marginBottom: "1rem" }}>
                  <div style={{ fontSize: "0.875rem", color: "var(--color-bc-text)", marginBottom: "0.375rem", fontWeight: 500 }}>
                    #{ch.name}
                  </div>
                  <NotificationLevelPicker
                    value={chLevel}
                    onChange={(level) => setChannelNotificationOverride(community.id, ch.id, level)}
                  />
                </div>
              );
            })}
        </>
      )}
    </div>
  );
}

// ── DM mailbox tab ────────────────────────────────────────────────────────────

function DmMailboxTab({ community }: { community: CommunityInfo }) {
  const [currentMailbox, setCurrentMailbox] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const seedNode = community.seed_nodes[0] ?? null;
  const isPreferred = !!currentMailbox && currentMailbox === seedNode;

  useEffect(() => {
    rpcClient.nodeGetConfig().then((cfg) => {
      setCurrentMailbox(cfg.preferred_mailbox_node);
      setLoading(false);
    }).catch(() => setLoading(false));
  }, []);

  const handleSet = async () => {
    if (!seedNode) return;
    setSaving(true);
    try {
      await rpcClient.dmSetPreferredMailboxCommunity({ community_id: community.id });
      setCurrentMailbox(seedNode);
      toast("Preferred DM mailbox updated and announced to the network.");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Failed to set preferred mailbox.", "error");
    } finally {
      setSaving(false);
    }
  };

  const handleClear = async () => {
    setSaving(true);
    try {
      await rpcClient.dmClearPreferredMailbox();
      setCurrentMailbox(null);
      toast("Preferred DM mailbox cleared.");
    } catch {
      toast("Failed to clear preferred mailbox.", "error");
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem" }}>Loading…</p>;
  }

  return (
    <div>
      <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", marginTop: 0, marginBottom: "1.5rem" }}>
        Set this community's node as your preferred DM mailbox. Your node will announce this preference to the DHT network so senders can find your mailbox directly, even before your first DM arrives.
      </p>

      {/* Current status */}
      <div style={{
        border: `1px solid ${currentMailbox ? "rgba(255,255,255,0.08)" : "rgba(255,255,255,0.05)"}`,
        borderRadius: "8px",
        padding: "1rem 1.25rem",
        marginBottom: "1.5rem",
        background: "var(--color-bc-surface-3)",
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: "0.625rem", marginBottom: "0.375rem" }}>
          <Mailbox size={16} style={{ color: currentMailbox ? "var(--color-bc-success, #57f287)" : "var(--color-bc-muted)", flexShrink: 0 }} />
          <span style={{ fontWeight: 600, fontSize: "0.875rem", color: "var(--color-bc-text)" }}>
            Current Preferred Mailbox
          </span>
        </div>
        {currentMailbox ? (
          <p style={{ margin: 0, fontFamily: "monospace", fontSize: "0.8125rem", color: "var(--color-bc-text)" }}>
            {currentMailbox}
            {isPreferred && (
              <span style={{ marginLeft: "0.5rem", fontSize: "0.75rem", color: "var(--color-bc-success, #57f287)", fontFamily: "inherit" }}>
                (this community)
              </span>
            )}
          </p>
        ) : (
          <p style={{ margin: 0, fontSize: "0.875rem", color: "var(--color-bc-muted)" }}>
            Not set — mailbox node is determined automatically by the DHT.
          </p>
        )}
      </div>

      {/* Actions */}
      {!seedNode ? (
        <p style={{ color: "var(--color-bc-warning, #f0b90b)", fontSize: "0.875rem" }}>
          This community has no seed node configured. Set one in the Seed Node tab first.
        </p>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: "0.75rem" }}>
          <div style={{
            border: `1px solid ${isPreferred ? "var(--color-bc-accent)" : "rgba(255,255,255,0.08)"}`,
            borderRadius: "8px",
            padding: "1rem 1.25rem",
            background: isPreferred ? "rgba(88,101,242,0.07)" : "var(--color-bc-surface-3)",
          }}>
            <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: "1rem" }}>
              <div>
                <div style={{ fontWeight: 600, fontSize: "0.9375rem", color: "var(--color-bc-text)", marginBottom: "0.25rem" }}>
                  Use {community.name}'s node
                </div>
                <div style={{ fontSize: "0.8125rem", color: "var(--color-bc-muted)", fontFamily: "monospace" }}>
                  {seedNode}
                </div>
              </div>
              {isPreferred ? (
                <button
                  onClick={() => void handleClear()}
                  disabled={saving}
                  style={{
                    flexShrink: 0,
                    padding: "0.375rem 0.875rem",
                    background: "transparent",
                    border: "1px solid rgba(255,255,255,0.12)",
                    borderRadius: "4px",
                    color: "var(--color-bc-muted)",
                    cursor: saving ? "not-allowed" : "pointer",
                    fontSize: "0.875rem",
                    opacity: saving ? 0.6 : 1,
                  }}
                >
                  {saving ? "Clearing…" : "Clear"}
                </button>
              ) : (
                <button
                  onClick={() => void handleSet()}
                  disabled={saving}
                  style={{
                    flexShrink: 0,
                    padding: "0.375rem 0.875rem",
                    background: "var(--color-bc-accent)",
                    border: "none",
                    borderRadius: "4px",
                    color: "#fff",
                    cursor: saving ? "not-allowed" : "pointer",
                    fontWeight: 600,
                    fontSize: "0.875rem",
                    opacity: saving ? 0.6 : 1,
                  }}
                >
                  {saving ? "Setting…" : "Set as Preferred"}
                </button>
              )}
            </div>
          </div>
          {currentMailbox && !isPreferred && (
            <button
              onClick={() => void handleClear()}
              disabled={saving}
              style={{
                alignSelf: "flex-start",
                padding: "0.375rem 0.75rem",
                background: "transparent",
                border: "1px solid rgba(255,255,255,0.1)",
                borderRadius: "4px",
                color: "var(--color-bc-muted)",
                cursor: saving ? "not-allowed" : "pointer",
                fontSize: "0.8125rem",
              }}
            >
              Clear current preference ({currentMailbox})
            </button>
          )}
        </div>
      )}
      <p style={{ margin: "1.25rem 0 0", fontSize: "0.8125rem", color: "var(--color-bc-muted)", lineHeight: 1.5 }}>
        Clearing only stops re-announcing your preference — it does not send a retraction to the network. Existing DHT records on other nodes expire naturally within 24 hours. To override sooner, set a different community's node as your preferred mailbox.
      </p>
    </div>
  );
}

// ── Danger zone tab ───────────────────────────────────────────────────────────

function DangerTab({ community }: { community: CommunityInfo }) {
  const navigate = useNavigate();
  const { identity } = useIdentityStore();
  const { removeCommunity } = useCommunitiesStore();
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isAdmin = identity ? community.admin_ids.includes(identity.peer_id) : false;

  const handleLeave = async () => {
    try {
      await rpcClient.communityLeave(community.id);
      removeCommunity(community.id);
      navigate("/app");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to leave community");
    }
  };

  return (
    <div>
      <div style={{ border: "1px solid var(--color-bc-danger)", borderRadius: "6px", padding: "1rem" }}>
        <div style={{ display: "flex", gap: "0.75rem", alignItems: "flex-start", marginBottom: "0.75rem" }}>
          <AlertTriangle size={20} style={{ color: "var(--color-bc-danger)", flexShrink: 0, marginTop: "2px" }} />
          <div>
            <div style={{ fontWeight: 600, color: "var(--color-bc-text)", marginBottom: "0.25rem" }}>
              {isAdmin ? "Delete Community" : "Leave Community"}
            </div>
            <div style={{ fontSize: "0.875rem", color: "var(--color-bc-muted)" }}>
              {isAdmin
                ? "This will permanently delete the community and all its channels. This action cannot be undone."
                : "You will lose access to this community's channels."}
            </div>
          </div>
        </div>
        {!confirming ? (
          <button
            onClick={() => setConfirming(true)}
            disabled={!community.reachable}
            title={!community.reachable ? "Seed node unreachable" : undefined}
            style={{ padding: "0.5rem 1rem", background: "var(--color-bc-danger)", border: "none", borderRadius: "4px", color: "#fff", cursor: !community.reachable ? "not-allowed" : "pointer", opacity: !community.reachable ? 0.4 : 1, fontWeight: 600, fontSize: "0.875rem" }}
          >
            {isAdmin ? "Delete Community" : "Leave Community"}
          </button>
        ) : (
          <div style={{ display: "flex", gap: "0.75rem", alignItems: "center" }}>
            <span style={{ fontSize: "0.875rem", color: "var(--color-bc-text)" }}>Are you sure?</span>
            <button
              onClick={() => void handleLeave()}
              style={{ padding: "0.375rem 0.875rem", background: "var(--color-bc-danger)", border: "none", borderRadius: "4px", color: "#fff", cursor: "pointer", fontWeight: 600, fontSize: "0.875rem" }}
            >
              Yes, {isAdmin ? "Delete" : "Leave"}
            </button>
            <button
              onClick={() => setConfirming(false)}
              style={{ padding: "0.375rem 0.875rem", background: "transparent", border: "1px solid rgba(255,255,255,0.12)", borderRadius: "4px", color: "var(--color-bc-text)", cursor: "pointer", fontSize: "0.875rem" }}
            >
              Cancel
            </button>
          </div>
        )}
        {error && <p style={{ color: "var(--color-bc-danger)", margin: "0.75rem 0 0", fontSize: "0.875rem" }}>{error}</p>}
      </div>
    </div>
  );
}

// ── CommunitySettingsPage ─────────────────────────────────────────────────────

export function CommunitySettingsPage() {
  const navigate = useNavigate();
  const { cid } = useParams<{ cid: string }>();
  const { communities, channels, loadChannels } = useCommunitiesStore();
  const { identity } = useIdentityStore();
  const isAdmin = identity && communities.find((c) => c.id === cid)
    ? communities.find((c) => c.id === cid)!.admin_ids.includes(identity.peer_id)
    : false;
  const TABS = isAdmin ? ADMIN_TABS : MEMBER_TABS;
  const [activeTab, setActiveTab] = useState<SettingsTab>("notifications");

  const community = communities.find((c) => c.id === cid);

  useEffect(() => {
    if (cid && !channels[cid]) void loadChannels(cid);
  }, [cid, channels, loadChannels]);

  // Default to "general" for admins on first load
  useEffect(() => {
    if (isAdmin) setActiveTab("general");
  }, [isAdmin]);

  if (!community) {
    return (
      <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "var(--color-bc-muted)" }}>
        Community not found.
      </div>
    );
  }

  return (
    <div style={{ display: "flex", height: "100%", background: "var(--color-bc-surface-3)", overflow: "hidden" }}>
      {/* Left nav */}
      <aside
        style={{
          width: "220px",
          background: "var(--color-bc-surface-2)",
          flexShrink: 0,
          padding: "1.5rem 0.5rem",
          overflowY: "auto",
        }}
      >
        <button
          onClick={() => navigate(`/app/community/${community.id}/channel/`)}
          style={{
            display: "flex",
            alignItems: "center",
            gap: "0.375rem",
            background: "none",
            border: "none",
            color: "var(--color-bc-muted)",
            cursor: "pointer",
            fontSize: "0.875rem",
            marginBottom: "1.25rem",
            padding: "0.25rem 0.5rem",
          }}
        >
          <ArrowLeft size={14} aria-hidden="true" />
          Back to {community.name}
        </button>

        <div style={{ fontSize: "0.6875rem", fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--color-bc-muted)", padding: "0 0.5rem 0.5rem" }}>
          Community Settings
        </div>

        {TABS.map((tab) => (
          <button
            key={tab.key}
            onClick={() => setActiveTab(tab.key)}
            style={{
              display: "block",
              width: "100%",
              padding: "0.5rem 0.75rem",
              background: activeTab === tab.key ? "var(--color-bc-surface-hover)" : "none",
              border: "none",
              color: tab.key === "danger" ? "var(--color-bc-danger)" : activeTab === tab.key ? "var(--color-bc-text)" : "var(--color-bc-muted)",
              cursor: "pointer",
              fontSize: "0.9375rem",
              textAlign: "left",
              borderRadius: "4px",
              fontWeight: activeTab === tab.key ? 600 : 400,
            }}
          >
            {tab.label}
          </button>
        ))}
      </aside>

      {/* Content */}
      <main style={{ flex: 1, overflowY: "auto", padding: "2rem" }}>
        {!community.reachable && (
          <div style={{
            padding: "0.75rem 1rem",
            marginBottom: "1.25rem",
            background: "rgba(240,185,11,0.12)",
            border: "1px solid rgba(240,185,11,0.3)",
            borderRadius: "6px",
            display: "flex",
            alignItems: "center",
            gap: "0.5rem",
            color: "var(--color-bc-warning, #f0b90b)",
            fontSize: "0.875rem",
            fontWeight: 500,
          }}>
            <AlertTriangle size={16} />
            <span>Seed peer is disconnected — community settings are read-only until the connection is restored.</span>
          </div>
        )}
        <h1 style={{ margin: "0 0 1.5rem", fontSize: "1.25rem", fontWeight: 700, color: "var(--color-bc-text)" }}>
          {TABS.find((t) => t.key === activeTab)?.label}
        </h1>
        {activeTab === "general" && <GeneralTab community={community} />}
        {activeTab === "seed_nodes" && <SeedNodesTab community={community} />}
        {activeTab === "admins" && <AdminsTab community={community} />}
        {activeTab === "channels" && <ChannelsTab community={community} />}
        {activeTab === "notifications" && <NotificationsTab community={community} />}
        {activeTab === "dm" && <DmMailboxTab community={community} />}
        {activeTab === "danger" && <DangerTab community={community} />}
      </main>
    </div>
  );
}
