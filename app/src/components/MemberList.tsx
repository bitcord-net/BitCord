import React, { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { MessageSquare } from "lucide-react";
import type { MemberInfo, RoleDto } from "../lib/rpc-types";
import { useCommunitiesStore } from "../store/communities";
import { usePresenceStore } from "../store/presence";
import { useIdentityStore } from "../store/identity";
import { rpcClient, useRpc } from "../hooks/useRpc";
import { PresenceIndicator } from "./PresenceIndicator";
import { toast } from "../store/toast";

const ROLE_BADGE: Record<RoleDto, { label: string; color: string }> = {
  admin: { label: "Admin", color: "var(--color-bc-accent)" },
  moderator: { label: "Mod", color: "var(--color-bc-warning)" },
  member: { label: "", color: "" },
};

interface Props {
  communityId: string;
}

function MemberRow({
  member,
  isCurrentUser,
  canModerate,
  isAdmin,
  communityId,
  status,
}: {
  member: MemberInfo;
  isCurrentUser: boolean;
  canModerate: boolean;
  isAdmin: boolean;
  communityId: string;
  status: string;
}) {
  const navigate = useNavigate();
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });

  const initials = member.display_name
    .split(/\s+/)
    .map((w) => w[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();

  const topRole: RoleDto = member.roles.includes("admin")
    ? "admin"
    : member.roles.includes("moderator")
    ? "moderator"
    : "member";

  const badge = ROLE_BADGE[topRole];

  const openMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    setMenuPos({ x: e.clientX, y: e.clientY });
    setMenuOpen(true);
  };

  const handleKick = async () => {
    setMenuOpen(false);
    try {
      await rpcClient.memberKick({ community_id: communityId, user_id: member.user_id });
      toast(`${member.display_name} was kicked.`, "info");
      void useCommunitiesStore.getState().loadMembers(communityId);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      toast(`Failed to kick: ${msg}`, "error");
    }
  };

  const handleBan = async () => {
    setMenuOpen(false);
    try {
      await rpcClient.memberBan({ community_id: communityId, user_id: member.user_id });
      toast(`${member.display_name} was banned.`, "warn");
      void useCommunitiesStore.getState().loadMembers(communityId);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      toast(`Failed to ban: ${msg}`, "error");
    }
  };

  const handleSetRole = async (role: RoleDto) => {
    setMenuOpen(false);
    try {
      await rpcClient.memberUpdateRole({ community_id: communityId, user_id: member.user_id, role });
      toast(`${member.display_name} is now ${role}.`, "info");
      void useCommunitiesStore.getState().loadMembers(communityId);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      toast(`Failed to update role: ${msg}`, "error");
    }
  };

  return (
    <>
      <div
        onContextMenu={!isCurrentUser ? openMenu : undefined}
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.5rem",
          padding: "0.3125rem 0.5rem",
          borderRadius: "4px",
          cursor: !isCurrentUser ? "context-menu" : "default",
        }}
        onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
        onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "transparent")}
      >
        {/* Avatar */}
        <div style={{ position: "relative", flexShrink: 0 }}>
          <div
            aria-hidden="true"
            style={{
              width: "32px",
              height: "32px",
              borderRadius: "50%",
              background: "var(--color-bc-surface-3)",
              color: "var(--color-bc-text)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 700,
              fontSize: "0.75rem",
            }}
          >
            {initials}
          </div>
          {/* Presence dot */}
          <div style={{ position: "absolute", bottom: 0, right: 0 }}>
            <PresenceIndicator status={status} size={10} borderColor="var(--color-bc-surface-2)" />
          </div>
        </div>

        {/* Name + role */}
        <div style={{ flex: 1, overflow: "hidden" }}>
          <div
            style={{
              fontSize: "0.875rem",
              fontWeight: 500,
              color: isCurrentUser ? "var(--color-bc-accent)" : "var(--color-bc-text)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {member.display_name}
            {isCurrentUser && " (you)"}
          </div>
          {badge.label && (
            <div style={{ fontSize: "0.6875rem", color: badge.color, fontWeight: 600 }}>
              {badge.label}
            </div>
          )}
        </div>

        {/* DM button (hover) */}
        {!isCurrentUser && (
          <button
            onClick={() => navigate(`/app/dm/${member.user_id}`)}
            title="Send Direct Message"
            aria-label={`Send DM to ${member.display_name}`}
            style={{
              background: "none",
              border: "none",
              color: "var(--color-bc-muted)",
              cursor: "pointer",
              padding: "3px",
              display: "flex",
              borderRadius: "3px",
              opacity: 0,
              transition: "opacity 0.1s",
            }}
            onFocus={(e) => ((e.currentTarget as HTMLElement).style.opacity = "1")}
            onBlur={(e) => ((e.currentTarget as HTMLElement).style.opacity = "0")}
          >
            <MessageSquare size={14} aria-hidden="true" />
          </button>
        )}
      </div>

      {menuOpen && (
        <MemberContextMenu
          member={member}
          x={menuPos.x}
          y={menuPos.y}
          isAdmin={isAdmin}
          canModerate={canModerate}
          onClose={() => setMenuOpen(false)}
          onDm={() => { setMenuOpen(false); navigate(`/app/dm/${member.user_id}`); }}
          onKick={handleKick}
          onBan={handleBan}
          onSetRole={handleSetRole}
        />
      )}
    </>
  );
}

function MemberContextMenu({
  member,
  x,
  y,
  isAdmin,
  canModerate,
  onClose,
  onDm,
  onKick,
  onBan,
  onSetRole,
}: {
  member: MemberInfo;
  x: number;
  y: number;
  isAdmin: boolean;
  canModerate: boolean;
  onClose: () => void;
  onDm: () => void;
  onKick: () => void;
  onBan: () => void;
  onSetRole: (role: RoleDto) => void;
}) {
  const menuRef = (el: HTMLDivElement | null) => {
    if (!el) return;
    const handler = (e: MouseEvent) => {
      if (!el.contains(e.target as Node)) onClose();
    };
    setTimeout(() => document.addEventListener("mousedown", handler), 0);
  };

  const itemStyle: React.CSSProperties = {
    display: "block",
    width: "100%",
    padding: "0.5rem 0.75rem",
    background: "none",
    border: "none",
    cursor: "pointer",
    fontSize: "0.875rem",
    textAlign: "left",
    borderRadius: "3px",
  };

  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label={`Actions for ${member.display_name}`}
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
        minWidth: "160px",
      }}
    >
      <div style={{ padding: "0.375rem 0.75rem 0.25rem", borderBottom: "1px solid rgba(255,255,255,0.06)", marginBottom: "0.25rem" }}>
        <div style={{ fontWeight: 600, color: "var(--color-bc-text)", fontSize: "0.875rem" }}>{member.display_name}</div>
        <div style={{ fontSize: "0.75rem", color: "var(--color-bc-muted)", fontFamily: "monospace", overflow: "hidden", textOverflow: "ellipsis" }}>
          {member.user_id.slice(0, 20)}…
        </div>
      </div>
      <button
        role="menuitem"
        onClick={onDm}
        style={{ ...itemStyle, color: "var(--color-bc-text)" }}
        onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
        onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
      >
        Message
      </button>
      {canModerate && (
        <button
          role="menuitem"
          onClick={onKick}
          style={{ ...itemStyle, color: "var(--color-bc-warning)" }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "rgba(250,166,26,0.1)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
        >
          Kick
        </button>
      )}
      {isAdmin && (
        <button
          role="menuitem"
          onClick={onBan}
          style={{ ...itemStyle, color: "var(--color-bc-danger)" }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "rgba(237,66,69,0.12)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
        >
          Ban
        </button>
      )}
      {isAdmin && (
        <>
          <div style={{ borderTop: "1px solid rgba(255,255,255,0.06)", margin: "0.25rem 0" }} />
          <div style={{ padding: "0.25rem 0.75rem 0.125rem", fontSize: "0.6875rem", fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--color-bc-muted)" }}>
            Set Role
          </div>
          {(["admin", "moderator", "member"] as RoleDto[])
            .filter((r) => !member.roles.includes(r))
            .map((r) => (
              <button
                key={r}
                role="menuitem"
                onClick={() => onSetRole(r)}
                style={{ ...itemStyle, color: "var(--color-bc-text)" }}
                onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
                onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
              >
                {r.charAt(0).toUpperCase() + r.slice(1)}
              </button>
            ))}
        </>
      )}
    </div>
  );
}

export function MemberList({ communityId }: Props) {
  const { members, loadMembers } = useCommunitiesStore();
  const { getStatus } = usePresenceStore();
  const { identity } = useIdentityStore();
  const communities = useCommunitiesStore((s) => s.communities);
  const { isConnected } = useRpc();

  useEffect(() => {
    if (!communityId || !isConnected) return;
    if (!members[communityId]) {
      void loadMembers(communityId);
    }
  }, [communityId, isConnected, members, loadMembers]);

  const community = communities.find((c) => c.id === communityId);
  const memberList = members[communityId] ?? [];

  const callerRoles = identity
    ? (memberList.find((m) => m.user_id === identity.peer_id)?.roles ?? [])
    : [];
  const isAdminOrMod = callerRoles.some((r) => r === "admin" || r === "moderator");
  const isAdmin = callerRoles.includes("admin");

  const withStatus = memberList.map((m) => ({
    member: m,
    status: getStatus(m.user_id),
  }));

  const online = withStatus.filter((x) => x.status !== "offline" && x.status !== "invisible");
  const offline = withStatus.filter((x) => x.status === "offline" || x.status === "invisible");

  if (!community) {
    return (
      <aside
        aria-label="Members"
        style={{ width: "240px", background: "var(--color-bc-surface-2)", flexShrink: 0, overflowY: "auto", padding: "1rem 0.75rem" }}
      >
        <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem" }}>Select a community.</p>
      </aside>
    );
  }

  const sectionHeaderStyle: React.CSSProperties = {
    fontSize: "0.6875rem",
    fontWeight: 700,
    textTransform: "uppercase",
    letterSpacing: "0.06em",
    color: "var(--color-bc-muted)",
    padding: "0.625rem 0.5rem 0.25rem",
  };

  return (
    <aside
      aria-label="Members"
      style={{
        width: "240px",
        background: "var(--color-bc-surface-2)",
        flexShrink: 0,
        overflowY: "auto",
        padding: "0.5rem 0",
      }}
    >
      {online.length > 0 && (
        <>
          <div style={sectionHeaderStyle}>Online — {online.length}</div>
          {online.map(({ member, status }) => (
            <MemberRow
              key={member.user_id}
              member={member}
              status={status}
              isCurrentUser={member.user_id === identity?.peer_id}
              canModerate={isAdminOrMod}
              isAdmin={isAdmin}
              communityId={communityId}
            />
          ))}
        </>
      )}
      {offline.length > 0 && (
        <>
          <div style={sectionHeaderStyle}>Offline — {offline.length}</div>
          {offline.map(({ member, status }) => (
            <MemberRow
              key={member.user_id}
              member={member}
              status={status}
              isCurrentUser={member.user_id === identity?.peer_id}
              canModerate={isAdminOrMod}
              isAdmin={isAdmin}
              communityId={communityId}
            />
          ))}
        </>
      )}
      {memberList.length === 0 && (
        <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", padding: "0.75rem 0.5rem" }}>
          No members loaded.
        </p>
      )}
    </aside>
  );
}
