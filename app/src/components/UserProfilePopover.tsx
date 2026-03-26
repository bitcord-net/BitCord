import { useEffect, useRef } from "react";
import { MessageSquare } from "lucide-react";
import { useNavigate } from "react-router-dom";
import type { MemberInfo } from "../lib/rpc-types";
import { PresenceIndicator } from "./PresenceIndicator";
import { usePresenceStore } from "../store/presence";
import { useCommunitiesStore } from "../store/communities";
import { useIdentityStore } from "../store/identity";

interface Props {
  member: MemberInfo;
  anchorRect: DOMRect;
  onClose: () => void;
}

export function UserProfilePopover({ member, anchorRect, onClose }: Props) {
  const navigate = useNavigate();
  const popoverRef = useRef<HTMLDivElement>(null);
  const { getStatus } = usePresenceStore();
  const { identity } = useIdentityStore();
  const { communities, members: allMembers } = useCommunitiesStore();

  const status = getStatus(member.user_id);
  const isCurrentUser = identity?.peer_id === member.user_id;

  // Mutual communities
  const mutualCommunities = communities.filter((c) => {
    const memberList = allMembers[c.id] ?? [];
    return memberList.some((m) => m.user_id === member.user_id);
  });

  const initials = member.display_name
    .split(/\s+/)
    .map((w) => w[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();

  // Position: try to appear to the left of the anchor; fallback to right
  const popoverWidth = 240;
  const left = anchorRect.left - popoverWidth - 8;
  const top = Math.min(anchorRect.top, window.innerHeight - 260);
  const finalLeft = left < 8 ? anchorRect.right + 8 : left;

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (!popoverRef.current?.contains(e.target as Node)) onClose();
    };
    setTimeout(() => document.addEventListener("mousedown", handler), 0);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose]);

  const handleDm = () => {
    onClose();
    navigate(`/app/dm/${member.user_id}`);
  };

  const shortId = member.user_id.length > 20
    ? `${member.user_id.slice(0, 10)}…${member.user_id.slice(-10)}`
    : member.user_id;

  return (
    <div
      ref={popoverRef}
      role="dialog"
      aria-label={`Profile: ${member.display_name}`}
      style={{
        position: "fixed",
        top,
        left: finalLeft,
        width: `${popoverWidth}px`,
        background: "var(--color-bc-surface-1)",
        border: "1px solid rgba(255,255,255,0.06)",
        borderRadius: "8px",
        boxShadow: "0 8px 32px rgba(0,0,0,0.6)",
        zIndex: 400,
        overflow: "hidden",
      }}
    >
      {/* Banner / Avatar area */}
      <div
        style={{
          background: "var(--color-bc-accent)",
          height: "60px",
          position: "relative",
        }}
      />
      <div
        style={{
          padding: "0 1rem 1rem",
          marginTop: "-24px",
        }}
      >
        {/* Avatar with presence */}
        <div style={{ position: "relative", display: "inline-block", marginBottom: "0.5rem" }}>
          <div
            aria-hidden="true"
            style={{
              width: "48px",
              height: "48px",
              borderRadius: "50%",
              background: "var(--color-bc-surface-3)",
              color: "var(--color-bc-text)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 700,
              fontSize: "1rem",
              border: "3px solid var(--color-bc-surface-1)",
            }}
          >
            {initials}
          </div>
          <div style={{ position: "absolute", bottom: 2, right: 2 }}>
            <PresenceIndicator status={status} size={12} borderColor="var(--color-bc-surface-1)" />
          </div>
        </div>

        {/* Name */}
        <div
          style={{
            fontWeight: 700,
            fontSize: "1rem",
            color: "var(--color-bc-text)",
            marginBottom: "2px",
          }}
        >
          {member.display_name}
          {isCurrentUser && (
            <span style={{ fontSize: "0.75rem", color: "var(--color-bc-muted)", fontWeight: 400, marginLeft: "0.25rem" }}>
              (you)
            </span>
          )}
        </div>

        {/* PeerId */}
        <div
          title={member.user_id}
          style={{
            fontSize: "0.6875rem",
            color: "var(--color-bc-muted)",
            fontFamily: "monospace",
            marginBottom: "0.75rem",
            wordBreak: "break-all",
          }}
        >
          {shortId}
        </div>

        {/* Roles */}
        {member.roles.filter((r) => r !== "member").length > 0 && (
          <div style={{ marginBottom: "0.75rem" }}>
            <div style={{ fontSize: "0.6875rem", fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--color-bc-muted)", marginBottom: "0.25rem" }}>
              Roles
            </div>
            <div style={{ display: "flex", gap: "0.25rem", flexWrap: "wrap" }}>
              {member.roles.filter((r) => r !== "member").map((r) => (
                <span
                  key={r}
                  style={{
                    fontSize: "0.6875rem",
                    padding: "1px 6px",
                    borderRadius: "3px",
                    background: r === "admin" ? "rgba(88,101,242,0.25)" : "rgba(250,166,26,0.2)",
                    color: r === "admin" ? "var(--color-bc-accent)" : "var(--color-bc-warning)",
                    fontWeight: 600,
                  }}
                >
                  {r === "admin" ? "Admin" : "Mod"}
                </span>
              ))}
            </div>
          </div>
        )}

        {/* Mutual communities */}
        {mutualCommunities.length > 0 && (
          <div style={{ marginBottom: "0.75rem" }}>
            <div style={{ fontSize: "0.6875rem", fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--color-bc-muted)", marginBottom: "0.25rem" }}>
              Mutual Communities
            </div>
            <div style={{ fontSize: "0.8125rem", color: "var(--color-bc-text)" }}>
              {mutualCommunities.map((c) => c.name).join(", ")}
            </div>
          </div>
        )}

        {/* DM button */}
        {!isCurrentUser && (
          <button
            onClick={handleDm}
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              gap: "0.375rem",
              width: "100%",
              padding: "0.5rem",
              background: "var(--color-bc-accent)",
              color: "#fff",
              border: "none",
              borderRadius: "4px",
              cursor: "pointer",
              fontSize: "0.875rem",
              fontWeight: 600,
              transition: "opacity 0.15s",
            }}
            onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.opacity = "0.85")}
            onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.opacity = "1")}
          >
            <MessageSquare size={14} aria-hidden="true" />
            Send DM
          </button>
        )}
      </div>
    </div>
  );
}
