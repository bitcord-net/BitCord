import React, { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { Outlet, useNavigate, useParams, useLocation } from "react-router-dom";
import { MessageSquare, Settings, Plus, Globe, LogOut, Copy, Trash2, ArrowLeft, WifiOff, Users, X } from "lucide-react";
import { useRpc, rpcClient } from "../hooks/useRpc";
import { useSubscription } from "../hooks/useSubscription";
import { useOsNotifications, isInDnd } from "../hooks/useOsNotifications";
import { useTheme } from "../hooks/useTheme";
import { useIsMobile } from "../hooks/useIsMobile";
import { useSettingsStore } from "../store/settings";
import { useIdentityStore } from "../store/identity";
import { useCommunitiesStore } from "../store/communities";
import { useMessagesStore } from "../store/messages";
import { usePresenceStore } from "../store/presence";
import { useDmsStore } from "../store/dms";
import { CommunityHeader } from "../components/CommunityHeader";
import { ChannelList } from "../components/ChannelList";
import { MemberList } from "../components/MemberList";
import { DMList } from "../components/DMList";
import { StatusSelector } from "../components/StatusSelector";
import { CreateCommunityModal } from "../components/CreateCommunityModal";
import { JoinCommunityModal } from "../components/JoinCommunityModal";
import { Toaster } from "../components/Toaster";
import { toast } from "../store/toast";
import type { CommunityInfo, UserStatus } from "../lib/rpc-types";

// ── App initializer ───────────────────────────────────────────────────────────

const HEARTBEAT_INTERVAL_MS = 15_000;
const PRESENCE_EXPIRY_MS = 45_000; // mark offline after 3 missed heartbeats
const IDLE_AFTER_MS = 5 * 60 * 1000;

function AppInitializer() {
  const { isConnected } = useRpc();
  const { load: loadIdentity, identity, setStatus } = useIdentityStore();
  const { load: loadCommunities, loadChannels, loadMembers, removeChannel, removeCommunity, updateCommunity, setSyncProgress, communities } = useCommunitiesStore();
  const { update: updateMessage, tombstone, incrementUnread } = useMessagesStore();
  const { update: updatePresence, expireStale } = usePresenceStore();
  const { appendMessage, incrementUnread: dmIncrementUnread, upsertConversation } = useDmsStore();
  const { chid, peerId: activeDmPeerId } = useParams();
  const navigate = useNavigate();
  const location = useLocation();
  const { notifyMessage, notifyDm } = useOsNotifications();

  // ── Initial data load ───────────────────────────────────────────────────
  useEffect(() => {
    if (!isConnected) return;
    void loadIdentity();
    void loadCommunities();
  }, [isConnected, loadIdentity, loadCommunities]);

  // Load members for all communities so DM display names resolve after F5
  useEffect(() => {
    for (const c of communities) {
      void loadMembers(c.id);
    }
  }, [communities, loadMembers]);

  // ── Presence heartbeat ───────────────────────────────────────────────────
  const lastActivityRef = useRef<number>(Date.now());
  const reportedStatusRef = useRef<UserStatus>("online");

  useEffect(() => {
    const updateActivity = () => { lastActivityRef.current = Date.now(); };
    window.addEventListener("mousemove", updateActivity);
    window.addEventListener("keydown", updateActivity);
    window.addEventListener("click", updateActivity);
    return () => {
      window.removeEventListener("mousemove", updateActivity);
      window.removeEventListener("keydown", updateActivity);
      window.removeEventListener("click", updateActivity);
    };
  }, []);

  useEffect(() => {
    if (!isConnected || !identity) return;

    const tick = async () => {
      const idle = Date.now() - lastActivityRef.current;
      const userStatus = identity.status;
      const scheduledDnd = isInDnd(useSettingsStore.getState().dndSchedule);

      // Determine effective status, honouring manual invisible > DND schedule > manual DND > auto idle
      let effectiveStatus: UserStatus = userStatus;
      if (userStatus === "invisible") {
        effectiveStatus = "invisible";
      } else if (scheduledDnd) {
        // Schedule silently overrides other statuses (except invisible); not persisted to identity store
        effectiveStatus = "do_not_disturb";
      } else if (userStatus === "do_not_disturb") {
        effectiveStatus = "do_not_disturb";
      } else {
        if (idle >= IDLE_AFTER_MS) effectiveStatus = "idle";
        else effectiveStatus = "online";
      }

      // Restore to online on activity if was auto-idle/offline (not during scheduled DND)
      if (
        idle < IDLE_AFTER_MS &&
        reportedStatusRef.current === "idle" &&
        userStatus !== "do_not_disturb" &&
        userStatus !== "invisible" &&
        !scheduledDnd
      ) {
        effectiveStatus = "online";
        await setStatus("online").catch(() => {});
      }

      if (effectiveStatus !== reportedStatusRef.current) {
        reportedStatusRef.current = effectiveStatus;
        // Only persist auto transitions; schedule-DND and manual statuses are handled elsewhere
        if (effectiveStatus === "idle") {
          await setStatus(effectiveStatus).catch(() => {});
        }
      }

      try {
        await rpcClient.presenceHeartbeat({ status: effectiveStatus });
      } catch { /* ignore — connection may be down */ }
    };

    // Send immediately on connect
    void tick();
    const interval = setInterval(() => void tick(), HEARTBEAT_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [isConnected, identity, setStatus]);

  // ── Presence expiry sweep ────────────────────────────────────────────────
  useEffect(() => {
    if (!isConnected) return;
    const sweep = setInterval(() => expireStale(PRESENCE_EXPIRY_MS), 10_000);
    return () => clearInterval(sweep);
  }, [isConnected, expireStale]);

  // ── Push event subscriptions ─────────────────────────────────────────────
  useSubscription("message_new", (ev) => {
    if (ev.data.channel_id !== chid) {
      incrementUnread(ev.data.channel_id);
    }
    // Skip notifications for own messages or when the app is focused on this channel.
    const myId = useIdentityStore.getState().identity?.peer_id;
    const isOwnMessage = ev.data.author_id === myId;
    const isActiveChannel = ev.data.channel_id === chid && !document.hidden;
    if (!isOwnMessage && !isActiveChannel) {
      const { channels, communities } = useCommunitiesStore.getState();
      const channelList = channels[ev.data.community_id] ?? [];
      const channel = channelList.find((c) => c.id === ev.data.channel_id);
      const community = communities.find((c) => c.id === ev.data.community_id);
      const title = community?.name ?? "BitCord";
      const body = `${ev.data.author_name ?? "Someone"} posted in #${channel?.name ?? "unknown"}`;
      notifyMessage(ev.data.community_id, ev.data.channel_id, title, body);
    }
  });

  useSubscription("message_edited", (ev) => {
    updateMessage(ev.data.channel_id, ev.data.message_id, {
      edited_at: ev.data.timestamp,
      ...(ev.data.body != null ? { body: ev.data.body } : {}),
    });
  });

  useSubscription("message_deleted", (ev) => {
    tombstone(ev.data.channel_id, ev.data.message_id);
  });

  useSubscription("presence_changed", (ev) => {
    updatePresence(ev.data.user_id, ev.data.status, ev.data.last_seen);
  });

  useSubscription("channel_created", (ev) => {
    void loadChannels(ev.data.community_id);
  });

  useSubscription("channel_deleted", (ev) => {
    removeChannel(ev.data.community_id, ev.data.channel_id);
  });

  useSubscription("member_joined", (ev) => {
    void loadMembers(ev.data.community_id);
  });

  useSubscription("member_left", (ev) => {
    void loadMembers(ev.data.community_id);
  });

  useSubscription("member_role_updated", (ev) => {
    void loadMembers(ev.data.community_id);
  });

  useSubscription("community_manifest_updated", (ev) => {
    void rpcClient.communityGet(ev.data.community_id).then(updateCommunity).catch(() => {});
    void loadChannels(ev.data.community_id);
    void loadMembers(ev.data.community_id);
  });

  useSubscription("seed_status_changed", (ev) => {
    const { community_id } = ev.data;
    void rpcClient.communityGet(community_id).then(updateCommunity).catch(() => {});
  });

  useSubscription("community_deleted", (ev) => {
    const deletedId = ev.data.community_id;
    const reason: string | undefined = ev.data.reason;
    removeCommunity(deletedId);
    // Navigate away from the deleted community's routes so the chat window
    // doesn't remain on screen.
    if (location.pathname.startsWith(`/app/community/${deletedId}/`) ||
        location.pathname === `/app/community/${deletedId}`) {
      navigate("/app/dm/", { replace: true });
    }
    if (reason) {
      toast(reason, "error");
    } else {
      toast("A community you were in was deleted by its admin.", "warn");
    }
  });

  useSubscription("sync_progress", (ev) => {
    setSyncProgress(ev.data.channel_id, ev.data.progress);
  });

  useSubscription("dm_new", (ev) => {
    const msg = ev.data.message;
    const myId = useIdentityStore.getState().identity?.peer_id;
    const convPeerId = msg.author_id === myId ? msg.peer_id : msg.author_id;
    appendMessage(convPeerId, msg);
    // Resolve display name: community members → existing stored name → peer ID
    const allMembers = useCommunitiesStore.getState().members;
    let displayName: string | undefined;
    for (const list of Object.values(allMembers)) {
      const found = list.find((m) => m.user_id === convPeerId);
      if (found) { displayName = found.display_name; break; }
    }
    if (!displayName) {
      const existingConv = useDmsStore.getState().conversations.find((c) => c.peerId === convPeerId);
      if (existingConv?.displayName && !existingConv.displayName.endsWith("…")) {
        displayName = existingConv.displayName;
      }
    }
    upsertConversation(convPeerId, displayName ?? convPeerId, msg);
    if (convPeerId !== activeDmPeerId) {
      dmIncrementUnread(convPeerId);
    }
    // OS notification for incoming DMs (not own messages, not active DM, app not focused).
    const isOwnMessage = msg.author_id === myId;
    const isActiveDm = convPeerId === activeDmPeerId && !document.hidden;
    if (!isOwnMessage && !isActiveDm) {
      const conv = useDmsStore.getState().conversations.find((c) => c.peerId === convPeerId);
      const senderName = conv?.displayName ?? "Someone";
      const preview = msg.body.length > 100 ? msg.body.slice(0, 97) + "…" : msg.body;
      notifyDm(senderName, preview);
    }
  });

  return null;
}

// ── Community context menu ────────────────────────────────────────────────────

function CommunityContextMenu({
  community,
  x,
  y,
  onClose,
}: {
  community: CommunityInfo;
  x: number;
  y: number;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const { removeCommunity } = useCommunitiesStore();
  const { identity } = useIdentityStore();
  const isAdmin = identity ? community.admin_ids.includes(identity.peer_id) : false;
  const menuRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  useEffect(() => { onCloseRef.current = onClose; });

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) {
        onCloseRef.current();
      }
    };
    const timerId = setTimeout(() => document.addEventListener("mousedown", handler), 0);
    return () => {
      clearTimeout(timerId);
      document.removeEventListener("mousedown", handler);
    };
  }, []);

  const itemStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: "0.5rem",
    width: "100%",
    padding: "0.5rem 0.75rem",
    background: "none",
    border: "none",
    cursor: "pointer",
    fontSize: "0.875rem",
    textAlign: "left",
    borderRadius: "3px",
  };

  const handleSettings = () => {
    onClose();
    navigate(`/app/community/${community.id}/settings`);
  };

  const handleCopyInvite = async () => {
    onClose();
    try {
      const link = await rpcClient.communityGenerateInvite(community.id);
      await navigator.clipboard.writeText(link);
    } catch {
      // ignore
    }
  };

  const handleLeave = async () => {
    onClose();
    try {
      await rpcClient.communityLeave(community.id);
      removeCommunity(community.id);
      navigate("/app/dm/");
    } catch {
      // ignore
    }
  };

  const handleDelete = async () => {
    onClose();
    try {
      await rpcClient.communityDelete(community.id);
      removeCommunity(community.id);
      navigate("/app/dm/");
    } catch {
      // ignore
    }
  };

  return (
    <div
      ref={menuRef}
      role="menu"
      style={{
        position: "fixed",
        top: y,
        left: x,
        background: "var(--color-bc-surface-1)",
        borderRadius: "4px",
        boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
        zIndex: 200,
        padding: "0.25rem",
        border: "1px solid rgba(255,255,255,0.06)",
        minWidth: "180px",
      }}
    >
      <div
        style={{
          padding: "0.375rem 0.75rem 0.375rem",
          fontSize: "0.8125rem",
          fontWeight: 700,
          color: "var(--color-bc-text)",
          borderBottom: "1px solid rgba(255,255,255,0.06)",
          marginBottom: "0.25rem",
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {community.name}
      </div>
      <button
        role="menuitem"
        onClick={handleSettings}
        style={{ ...itemStyle, color: "var(--color-bc-text)" }}
        onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
        onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
      >
        <Settings size={14} aria-hidden="true" />
        Settings
      </button>
      <button
        role="menuitem"
        onClick={() => void handleCopyInvite()}
        style={{ ...itemStyle, color: "var(--color-bc-text)" }}
        onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
        onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
      >
        <Copy size={14} aria-hidden="true" />
        Copy Invite Link
      </button>
      <div
        aria-hidden="true"
        style={{ height: "1px", background: "rgba(255,255,255,0.06)", margin: "0.25rem 0" }}
      />
      <button
        role="menuitem"
        onClick={() => void (isAdmin ? handleDelete() : handleLeave())}
        style={{ ...itemStyle, color: "var(--color-bc-danger)" }}
        onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "rgba(237,66,69,0.12)")}
        onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
      >
        {isAdmin ? <Trash2 size={14} aria-hidden="true" /> : <LogOut size={14} aria-hidden="true" />}
        {isAdmin ? "Delete Community" : "Leave Community"}
      </button>
    </div>
  );
}

// ── Community icon button ─────────────────────────────────────────────────────

function CommunityButton({
  community,
  active,
  unread,
  onClick,
}: {
  community: CommunityInfo;
  active: boolean;
  unread: number;
  onClick: () => void;
}) {
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);

  const initials = community.name
    .split(/\s+/)
    .map((w) => w[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();

  return (
    <>
      <div style={{ position: "relative", flexShrink: 0 }}>
      <button
        onClick={onClick}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenuPos({ x: e.clientX, y: e.clientY });
        }}
        title={community.reachable ? community.name : `${community.name} (unreachable)`}
        aria-label={community.name}
        aria-pressed={active}
        style={{
          width: "48px",
          height: "48px",
          borderRadius: active ? "30%" : "50%",
          border: "none",
          background: active
            ? "var(--color-bc-accent)"
            : "var(--color-bc-surface-3)",
          color: active ? "#fff" : "var(--color-bc-muted)",
          fontWeight: 700,
          fontSize: "0.875rem",
          cursor: "pointer",
          transition: "border-radius 0.15s, background 0.15s",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          opacity: community.reachable ? 1 : 0.4,
        }}
      >
        {initials}
      </button>
      {unread > 0 && !active && (
        <div
          aria-label={`${unread} unread message${unread !== 1 ? "s" : ""} in ${community.name}`}
          style={{
            position: "absolute",
            bottom: 0,
            right: 0,
            background: "var(--color-bc-danger)",
            color: "#fff",
            fontSize: "0.625rem",
            fontWeight: 700,
            borderRadius: "8px",
            minWidth: "16px",
            height: "16px",
            border: "2px solid var(--color-bc-surface-1)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            padding: "0 3px",
            pointerEvents: "none",
          }}
        >
          {unread > 99 ? "99+" : unread}
        </div>
      )}
      </div>
      {menuPos && (
        <CommunityContextMenu
          community={community}
          x={menuPos.x}
          y={menuPos.y}
          onClose={() => setMenuPos(null)}
        />
      )}
    </>
  );
}

// ── Community sidebar ─────────────────────────────────────────────────────────

function CommunitySidebar() {
  const navigate = useNavigate();
  const location = useLocation();
  const { cid } = useParams();
  const { communities, channels, setActive, addCommunity, loadChannels, loadMembers } = useCommunitiesStore();
  const { serverEnabled } = useIdentityStore();
  const [showCreate, setShowCreate] = useState(false);
  const [showJoin, setShowJoin] = useState(false);

  const isDmActive = location.pathname.startsWith("/app/dm");
  const { conversations } = useDmsStore();
  const totalDmUnread = conversations.reduce((sum, c) => sum + c.unread, 0);
  const { unreadCounts } = useMessagesStore();

  const handleSelect = (id: string) => {
    setActive(id);
    void loadChannels(id);
    void loadMembers(id);
    navigate(`/app/community/${id}/channel/`);
  };

  const handleCreated = (community: CommunityInfo) => {
    addCommunity(community);
    setShowCreate(false);
    handleSelect(community.id);
  };

  const handleJoined = (community: CommunityInfo) => {
    // Re-fetch to get the authoritative reachable status at the moment we add the
    // community to the store.  The object returned by communityJoin has reachable=false
    // for seed-based communities, but by the time the "done" animation finishes the
    // seed may already be connected, so we ask the backend for the current state.
    void rpcClient.communityGet(community.id)
      .then((fresh) => addCommunity(fresh))
      .catch(() => addCommunity(community));
    setShowJoin(false);
    handleSelect(community.id);
  };

  return (
    <>
      <nav
        aria-label="Communities"
        data-tauri-drag-region
        style={{
          width: "72px",
          background: "var(--color-bc-surface-1)",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          padding: "0.75rem 0",
          gap: "0.5rem",
          flexShrink: 0,
          overflowY: "auto",
        }}
      >
        {/* DMs shortcut */}
        <div style={{ position: "relative", flexShrink: 0 }}>
          <button
            onClick={() => navigate("/app/dm/")}
            title="Direct Messages"
            aria-label="Direct Messages"
            aria-pressed={isDmActive}
            style={{
              width: "48px",
              height: "48px",
              borderRadius: isDmActive ? "30%" : "50%",
              border: "none",
              background: isDmActive ? "var(--color-bc-accent)" : "var(--color-bc-surface-3)",
              color: isDmActive ? "#fff" : "var(--color-bc-muted)",
              cursor: "pointer",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              transition: "border-radius 0.15s, background 0.15s",
            }}
          >
            <MessageSquare size={22} />
          </button>
          {totalDmUnread > 0 && !isDmActive && (
            <div
              aria-label={`${totalDmUnread} unread direct message${totalDmUnread !== 1 ? "s" : ""}`}
              style={{
                position: "absolute",
                bottom: 0,
                right: 0,
                background: "var(--color-bc-danger)",
                color: "#fff",
                fontSize: "0.625rem",
                fontWeight: 700,
                borderRadius: "8px",
                minWidth: "16px",
                height: "16px",
                padding: "0 4px",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                boxSizing: "border-box",
                border: "2px solid var(--color-bc-surface-1)",
                pointerEvents: "none",
              }}
            >
              {totalDmUnread > 99 ? "99+" : totalDmUnread}
            </div>
          )}
        </div>

        {/* Separator */}
        <div
          aria-hidden="true"
          style={{
            width: "32px",
            height: "2px",
            borderRadius: "1px",
            background: "var(--color-bc-surface-3)",
          }}
        />

        {communities.map((c) => {
          const communityUnread = (channels[c.id] ?? []).reduce(
            (sum, ch) => sum + (unreadCounts[ch.id] ?? 0),
            0
          );
          return (
            <CommunityButton
              key={c.id}
              community={c}
              active={c.id === cid && !isDmActive}
              unread={communityUnread}
              onClick={() => handleSelect(c.id)}
            />
          );
        })}

        {/* Separator before add buttons */}
        {communities.length > 0 && (
          <div
            aria-hidden="true"
            style={{
              width: "32px",
              height: "2px",
              borderRadius: "1px",
              background: "var(--color-bc-surface-3)",
            }}
          />
        )}

        {/* Join community */}
        <button
          onClick={() => setShowJoin(true)}
          title="Join Community"
          aria-label="Join Community"
          style={{
            width: "48px",
            height: "48px",
            borderRadius: "50%",
            border: "none",
            background: "var(--color-bc-surface-3)",
            color: "var(--color-bc-success)",
            cursor: "pointer",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
          }}
        >
          <Plus size={22} />
        </button>

        {/* Create community */}
        <button
          onClick={() => setShowCreate(true)}
          title="Create Community"
          aria-label="Create Community"
          style={{
            width: "48px",
            height: "48px",
            borderRadius: "50%",
            border: "none",
            background: "var(--color-bc-surface-3)",
            color: "var(--color-bc-muted)",
            cursor: "pointer",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
          }}
        >
          <Globe size={20} />
        </button>
      </nav>

      {showCreate && (
        <CreateCommunityModal
          onClose={() => setShowCreate(false)}
          onCreated={handleCreated}
          serverEnabled={serverEnabled}
        />
      )}
      {showJoin && (
        <JoinCommunityModal
          onClose={() => setShowJoin(false)}
          onJoined={handleJoined}
        />
      )}
    </>
  );
}

// ── Channel panel (community mode) ────────────────────────────────────────────

function ChannelPanel({ isMobile }: { isMobile?: boolean }) {
  const { cid } = useParams();
  const { communities, channels, loadChannels } = useCommunitiesStore();
  const { isConnected } = useRpc();
  const community = cid ? communities.find((c) => c.id === cid) : null;

  // Auto-load channels when navigating directly to a community URL (e.g. after restart),
  // or when channels are empty but the community manifest lists channel IDs.
  useEffect(() => {
    if (!cid || !isConnected) return;
    const loaded = channels[cid];
    const hasUnloadedChannels =
      !loaded || (loaded.length === 0 && (community?.channel_ids.length ?? 0) > 0);
    if (hasUnloadedChannels) {
      void loadChannels(cid);
    }
  }, [cid, isConnected, channels, community, loadChannels]);

  return (
    <aside
      aria-label="Channels"
      style={{
        ...(isMobile ? { flex: 1 } : { width: "240px", flexShrink: 0 }),
        background: "var(--color-bc-surface-2)",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
    >
      {/* Community header */}
      {community ? (
        <CommunityHeader community={community} />
      ) : (
        <div
          style={{
            padding: "1rem",
            fontWeight: 700,
            borderBottom: "1px solid var(--color-bc-surface-1)",
            color: "var(--color-bc-text)",
            flexShrink: 0,
            height: "48px",
            display: "flex",
            alignItems: "center",
          }}
        >
          Select a community
        </div>
      )}

      {/* Channel list */}
      {cid ? (
        <ChannelList communityId={cid} />
      ) : (
        <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", padding: "1rem" }}>
          <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", textAlign: "center", margin: 0 }}>
            Select a community<br />to see channels.
          </p>
        </div>
      )}

      {/* User panel */}
      <UserPanel />
    </aside>
  );
}

// ── DM panel (DM mode) ────────────────────────────────────────────────────────

function DMPanel({ isMobile }: { isMobile?: boolean }) {
  return (
    <aside
      aria-label="Direct Messages"
      style={{
        ...(isMobile ? { flex: 1 } : { width: "240px", flexShrink: 0 }),
        background: "var(--color-bc-surface-2)",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
    >
      <DMList />
      <UserPanel />
    </aside>
  );
}

// ── User panel (bottom of channel/DM list) ────────────────────────────────────

function UserPanel() {
  const navigate = useNavigate();
  const { identity } = useIdentityStore();

  const initials = (identity?.display_name ?? "?")
    .split(/\s+/)
    .map((w) => w[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.5rem",
        padding: "0.625rem 0.75rem",
        background: "var(--color-bc-surface-1)",
        borderTop: "1px solid rgba(255,255,255,0.04)",
        flexShrink: 0,
      }}
    >
      <div
        style={{
          width: "32px",
          height: "32px",
          borderRadius: "50%",
          background: "var(--color-bc-accent)",
          color: "#fff",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontWeight: 700,
          fontSize: "0.75rem",
          flexShrink: 0,
        }}
        aria-hidden="true"
      >
        {initials}
      </div>
      <div style={{ flex: 1, overflow: "hidden", minWidth: 0 }}>
        <span
          style={{
            display: "block",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            fontWeight: 600,
            fontSize: "0.875rem",
            color: "var(--color-bc-text)",
          }}
        >
          {identity?.display_name ?? "Unknown"}
        </span>
      </div>
      {/* Status selector */}
      <StatusSelector />
      <button
        onClick={() => navigate("/app/settings")}
        title="Settings"
        aria-label="Open settings"
        style={{
          border: "none",
          background: "transparent",
          color: "var(--color-bc-muted)",
          cursor: "pointer",
          padding: "2px",
          display: "flex",
          borderRadius: "3px",
          transition: "color 0.15s",
        }}
        onMouseEnter={(e) =>
          ((e.currentTarget as HTMLButtonElement).style.color =
            "var(--color-bc-text)")
        }
        onMouseLeave={(e) =>
          ((e.currentTarget as HTMLButtonElement).style.color =
            "var(--color-bc-muted)")
        }
      >
        <Settings size={18} aria-hidden="true" />
      </button>
    </div>
  );
}

// ── Mobile channel header ─────────────────────────────────────────────────────

function MobileChannelHeader({
  title,
  onBack,
  onMembersClick,
}: {
  title: string;
  onBack: () => void;
  onMembersClick?: () => void;
}) {
  const btnStyle: React.CSSProperties = {
    width: "40px",
    height: "48px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: "none",
    border: "none",
    cursor: "pointer",
    color: "var(--color-bc-text)",
    flexShrink: 0,
  };
  return (
    <div
      data-tauri-drag-region
      style={{
        display: "flex",
        alignItems: "center",
        height: "48px",
        background: "var(--color-bc-surface-1)",
        borderBottom: "1px solid var(--color-bc-surface-3)",
        flexShrink: 0,
      }}
    >
      <button style={btnStyle} onClick={onBack} aria-label="Back">
        <ArrowLeft size={18} aria-hidden="true" />
      </button>
      <span
        style={{
          flex: 1,
          textAlign: "center",
          fontWeight: 600,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          color: "var(--color-bc-text)",
          fontSize: "0.9375rem",
        }}
      >
        {title}
      </span>
      {onMembersClick ? (
        <button style={btnStyle} onClick={onMembersClick} aria-label="Members">
          <Users size={18} aria-hidden="true" />
        </button>
      ) : (
        <div style={{ width: "40px", flexShrink: 0 }} />
      )}
    </div>
  );
}

// ── Mobile member list drawer ─────────────────────────────────────────────────

function MemberListDrawer({
  communityId,
  onClose,
}: {
  communityId: string;
  onClose: () => void;
}) {
  return (
    <>
      <div
        onClick={onClose}
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 40,
          background: "rgba(0,0,0,0.5)",
        }}
      />
      <div
        style={{
          position: "fixed",
          top: 0,
          right: 0,
          bottom: 0,
          width: "min(280px, 85vw)",
          zIndex: 41,
          background: "var(--color-bc-surface-2)",
          display: "flex",
          flexDirection: "column",
          boxShadow: "-4px 0 16px rgba(0,0,0,0.4)",
          paddingTop: "var(--sat, env(safe-area-inset-top, 0px))",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "0.75rem 1rem",
            borderBottom: "1px solid var(--color-bc-surface-3)",
            flexShrink: 0,
          }}
        >
          <span style={{ fontWeight: 600, color: "var(--color-bc-text)" }}>Members</span>
          <button
            onClick={onClose}
            aria-label="Close members"
            style={{
              background: "none",
              border: "none",
              cursor: "pointer",
              color: "var(--color-bc-muted)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              padding: "4px",
            }}
          >
            <X size={18} aria-hidden="true" />
          </button>
        </div>
        <div style={{ flex: 1, overflowY: "auto" }}>
          <MemberList communityId={communityId} />
        </div>
      </div>
    </>
  );
}

// ── Seed-unreachable banner ────────────────────────────────────────────────────

function SeedUnreachableBanner() {
  const { cid } = useParams();
  const { communities } = useCommunitiesStore();
  const community = cid ? communities.find((c) => c.id === cid) : null;

  if (!community || community.reachable) return null;

  return (
    <div
      role="alert"
      style={{
        padding: "7px 16px",
        background: "rgba(240,185,11,0.12)",
        borderBottom: "1px solid rgba(240,185,11,0.25)",
        color: "var(--color-bc-warning, #f0b90b)",
        fontSize: "0.8125rem",
        fontWeight: 600,
        display: "flex",
        alignItems: "center",
        gap: "8px",
        flexShrink: 0,
      }}
    >
      <WifiOff size={13} aria-hidden="true" />
      <span>
        Seed node unreachable — message history is available read-only. Sending messages, reactions,
        and other live actions are disabled until the connection is restored.
      </span>
    </div>
  );
}

// ── Main content area ─────────────────────────────────────────────────────────

function MainContent() {
  const { cid, chid } = useParams();
  const location = useLocation();
  const isDmRoute = location.pathname.startsWith("/app/dm");
  const isSettingsRoute = location.pathname.endsWith("/settings");

  // Settings and DM routes always just render Outlet
  if (isDmRoute || isSettingsRoute) {
    return (
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          background: "var(--color-bc-surface-3)",
          overflow: "hidden",
        }}
      >
        <Outlet />
      </div>
    );
  }

  if (!cid) {
    return (
      <div
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--color-bc-surface-3)",
          color: "var(--color-bc-muted)",
          flexDirection: "column",
          gap: "0.5rem",
        }}
      >
        <span style={{ fontSize: "1.5rem" }}>👋</span>
        <span>Select a community to get started.</span>
      </div>
    );
  }

  if (!chid) {
    return (
      <div
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--color-bc-surface-3)",
          color: "var(--color-bc-muted)",
        }}
      >
        Select a channel.
      </div>
    );
  }

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        background: "var(--color-bc-surface-3)",
        overflow: "hidden",
      }}
    >
      <Outlet />
    </div>
  );
}

// ── AppLayout root ────────────────────────────────────────────────────────────

export function AppLayout() {
  useTheme();
  const { cid, chid, peerId } = useParams();
  const location = useLocation();
  const navigate = useNavigate();
  const isDmRoute = location.pathname.startsWith("/app/dm");
  const isMobile = useIsMobile();
  const hasDestination = Boolean(chid || peerId || location.pathname.includes("/settings"));

  const { channels } = useCommunitiesStore();
  const { conversations } = useDmsStore();
  const [membersDrawerOpen, setMembersDrawerOpen] = useState(false);

  useEffect(() => {
    setMembersDrawerOpen(false);
  }, [chid, peerId]);

  const channelTitle = useMemo(() => {
    if (location.pathname.includes("/settings")) return "Settings";
    if (isDmRoute && peerId) {
      const conv = conversations.find((c) => c.peerId === peerId);
      return conv?.displayName ?? peerId;
    }
    if (cid && chid) {
      const chs = channels[cid];
      const ch = chs?.find((c) => c.id === chid);
      return ch?.name ? `# ${ch.name}` : "";
    }
    return "";
  }, [location.pathname, isDmRoute, peerId, conversations, cid, chid, channels]);

  const handleMobileBack = useCallback(() => {
    if (isDmRoute) navigate("/app/dm/");
    else if (cid) navigate(`/app/community/${cid}/channel/`);
    else navigate(-1 as never);
  }, [isDmRoute, cid, navigate]);

  if (isMobile) {
    return (
      <div
        style={{
          display: "flex",
          height: "100%",
          overflow: "hidden",
          background: "var(--color-bc-base)",
        }}
      >
        <AppInitializer />
        {hasDestination ? (
          <>
            <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minWidth: 0 }}>
              <MobileChannelHeader
                title={channelTitle}
                onBack={handleMobileBack}
                onMembersClick={cid && !isDmRoute ? () => setMembersDrawerOpen(true) : undefined}
              />
              {!isDmRoute && <SeedUnreachableBanner />}
              <MainContent />
            </div>
            {membersDrawerOpen && cid && (
              <MemberListDrawer communityId={cid} onClose={() => setMembersDrawerOpen(false)} />
            )}
          </>
        ) : (
          <>
            <CommunitySidebar />
            {isDmRoute ? (
              <DMPanel isMobile />
            ) : (
              <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minWidth: 0 }}>
                <SeedUnreachableBanner />
                <ChannelPanel isMobile />
              </div>
            )}
          </>
        )}
        <Toaster />
      </div>
    );
  }

  return (
    <div
      style={{
        display: "flex",
        height: "100%",
        overflow: "hidden",
        background: "var(--color-bc-base)",
      }}
    >
      <AppInitializer />
      <CommunitySidebar />
      {isDmRoute ? (
        <>
          <DMPanel />
          <MainContent />
        </>
      ) : (
        <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minWidth: 0 }}>
          <SeedUnreachableBanner />
          <div style={{ flex: 1, display: "flex", overflow: "hidden", minWidth: 0 }}>
            <ChannelPanel />
            <MainContent />
            {cid && <MemberList communityId={cid} />}
          </div>
        </div>
      )}
      <Toaster />
    </div>
  );
}
