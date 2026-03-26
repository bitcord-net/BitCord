import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Plus } from "lucide-react";
import { useDmsStore } from "../store/dms";
import { usePresenceStore } from "../store/presence";
import { useCommunitiesStore } from "../store/communities";
import { PresenceIndicator } from "./PresenceIndicator";
import { NewDMModal } from "./NewDMModal";
import { formatDistanceToNow } from "date-fns";

export function DMList() {
  const navigate = useNavigate();
  const { peerId: activePeerId } = useParams<{ peerId: string }>();
  const [showNewDm, setShowNewDm] = useState(false);
  const { conversations } = useDmsStore();
  const { getStatus } = usePresenceStore();
  const { members: allMembers } = useCommunitiesStore();

  // Build a peerId → displayName lookup from all community member lists
  const memberNameById = new Map<string, string>();
  for (const list of Object.values(allMembers)) {
    for (const m of list) {
      if (!memberNameById.has(m.user_id)) {
        memberNameById.set(m.user_id, m.display_name);
      }
    }
  }

  // Sort by last message time descending
  const sorted = [...conversations].sort((a, b) => {
    const ta = a.lastMessage ? new Date(a.lastMessage.timestamp).getTime() : 0;
    const tb = b.lastMessage ? new Date(b.lastMessage.timestamp).getTime() : 0;
    return tb - ta;
  });

  return (
    <>
      <div
        style={{
          padding: "0.625rem 0.75rem 0.375rem",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          flexShrink: 0,
        }}
      >
        <span
          style={{
            fontSize: "0.6875rem",
            fontWeight: 700,
            textTransform: "uppercase",
            letterSpacing: "0.06em",
            color: "var(--color-bc-muted)",
          }}
        >
          Direct Messages
        </span>
        <button
          onClick={() => setShowNewDm(true)}
          title="New Direct Message"
          aria-label="Start new direct message"
          style={{
            background: "none",
            border: "none",
            color: "var(--color-bc-muted)",
            cursor: "pointer",
            padding: "2px",
            display: "flex",
            borderRadius: "3px",
            transition: "color 0.15s",
          }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.color = "var(--color-bc-text)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.color = "var(--color-bc-muted)")}
        >
          <Plus size={16} aria-hidden="true" />
        </button>
      </div>

      <div style={{ flex: 1, overflowY: "auto" }}>
        {sorted.length === 0 ? (
          <p style={{ color: "var(--color-bc-muted)", fontSize: "0.8125rem", padding: "0.75rem", margin: 0 }}>
            No conversations yet.{" "}
            <button
              onClick={() => setShowNewDm(true)}
              style={{ background: "none", border: "none", color: "var(--color-bc-accent)", cursor: "pointer", padding: 0, fontSize: "inherit" }}
            >
              Start one
            </button>
          </p>
        ) : (
          sorted.map((conv) => {
            const status = getStatus(conv.peerId);
            const isActive = conv.peerId === activePeerId;
            const displayName = memberNameById.get(conv.peerId) ?? conv.displayName;
            const initials = displayName
              .split(/\s+/)
              .map((w) => w[0] ?? "")
              .join("")
              .slice(0, 2)
              .toUpperCase();

            return (
              <button
                key={conv.peerId}
                onClick={() => navigate(`/app/dm/${conv.peerId}`)}
                aria-current={isActive ? "page" : undefined}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "0.5rem",
                  width: "100%",
                  padding: "0.375rem 0.75rem",
                  background: isActive ? "var(--color-bc-surface-3)" : "none",
                  border: "none",
                  cursor: "pointer",
                  borderRadius: "4px",
                  textAlign: "left",
                }}
                onMouseEnter={(e) => {
                  if (!isActive) (e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)";
                }}
                onMouseLeave={(e) => {
                  if (!isActive) (e.currentTarget as HTMLElement).style.background = "none";
                }}
              >
                {/* Avatar with presence */}
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
                  <div style={{ position: "absolute", bottom: 0, right: 0 }}>
                    <PresenceIndicator status={status} size={10} borderColor="var(--color-bc-surface-2)" />
                  </div>
                </div>

                {/* Name + last message */}
                <div style={{ flex: 1, overflow: "hidden", minWidth: 0 }}>
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: "0.25rem" }}>
                    <span
                      style={{
                        fontSize: "0.875rem",
                        fontWeight: conv.unread > 0 ? 700 : 500,
                        color: "var(--color-bc-text)",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {displayName}
                    </span>
                    {conv.lastMessage && (
                      <span style={{ fontSize: "0.6875rem", color: "var(--color-bc-muted)", flexShrink: 0 }}>
                        {formatDistanceToNow(new Date(conv.lastMessage.timestamp), { addSuffix: false })}
                      </span>
                    )}
                  </div>
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: "0.25rem" }}>
                    <span
                      style={{
                        fontSize: "0.75rem",
                        color: "var(--color-bc-muted)",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                        flex: 1,
                      }}
                    >
                      {conv.lastMessage?.body ?? "No messages yet"}
                    </span>
                    {conv.unread > 0 && (
                      <span
                        aria-label={`${conv.unread} unread`}
                        style={{
                          background: "var(--color-bc-danger)",
                          color: "#fff",
                          fontSize: "0.6875rem",
                          fontWeight: 700,
                          borderRadius: "8px",
                          padding: "1px 5px",
                          flexShrink: 0,
                        }}
                      >
                        {conv.unread > 99 ? "99+" : conv.unread}
                      </span>
                    )}
                  </div>
                </div>
              </button>
            );
          })
        )}
      </div>

      {showNewDm && <NewDMModal onClose={() => setShowNewDm(false)} />}
    </>
  );
}
