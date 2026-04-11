import React, { useState, useEffect, useRef } from "react";
import { X, Search, UserPlus } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { useCommunitiesStore } from "../store/communities";
import { useIdentityStore } from "../store/identity";
import { usePresenceStore } from "../store/presence";
import { PresenceIndicator } from "./PresenceIndicator";

const PEER_ID_RE = /^[0-9a-f]{64}$/i;

interface Props {
  onClose: () => void;
}

export function NewDMModal({ onClose }: Props) {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const { members: allMembers } = useCommunitiesStore();
  const { identity } = useIdentityStore();
  const { getStatus } = usePresenceStore();

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Collect unique peers from all communities (excluding self)
  const knownPeers = (() => {
    const seen = new Set<string>();
    const peers: Array<{ user_id: string; display_name: string }> = [];
    for (const memberList of Object.values(allMembers)) {
      for (const m of memberList) {
        if (m.user_id !== identity?.peer_id && !seen.has(m.user_id)) {
          seen.add(m.user_id);
          peers.push({ user_id: m.user_id, display_name: m.display_name });
        }
      }
    }
    return peers;
  })();

  const trimmed = query.trim();
  const filtered = trimmed
    ? knownPeers.filter((p) =>
        p.display_name.toLowerCase().includes(trimmed.toLowerCase()) ||
        p.user_id.toLowerCase().includes(trimmed.toLowerCase())
      )
    : knownPeers;

  // If query is a valid peer ID not already in the known list, offer a direct-open option.
  const directPeerId =
    PEER_ID_RE.test(trimmed) && !knownPeers.some((p) => p.user_id.toLowerCase() === trimmed.toLowerCase())
      ? trimmed.toLowerCase()
      : null;

  const handleSelect = (peerId: string) => {
    onClose();
    navigate(`/app/dm/${peerId}`);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="New Direct Message"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.6)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 500,
      }}
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div
        style={{
          background: "var(--color-bc-surface-2)",
          borderRadius: "8px",
          width: "440px",
          maxHeight: "520px",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          boxShadow: "0 16px 48px rgba(0,0,0,0.6)",
        }}
        onKeyDown={handleKeyDown}
      >
        {/* Header */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "1rem 1.25rem 0.75rem",
            borderBottom: "1px solid rgba(255,255,255,0.06)",
          }}
        >
          <h2 style={{ margin: 0, fontSize: "1rem", fontWeight: 700, color: "var(--color-bc-text)" }}>
            New Message
          </h2>
          <button
            onClick={onClose}
            aria-label="Close"
            style={{ background: "none", border: "none", color: "var(--color-bc-muted)", cursor: "pointer", padding: "2px", display: "flex", borderRadius: "3px" }}
          >
            <X size={18} aria-hidden="true" />
          </button>
        </div>

        {/* Search input */}
        <div style={{ padding: "0.75rem 1.25rem", borderBottom: "1px solid rgba(255,255,255,0.06)" }}>
          <div style={{ position: "relative" }}>
            <Search
              size={14}
              aria-hidden="true"
              style={{
                position: "absolute",
                left: "0.625rem",
                top: "50%",
                transform: "translateY(-50%)",
                color: "var(--color-bc-muted)",
                pointerEvents: "none",
              }}
            />
            <input
              ref={inputRef}
              type="search"
              placeholder="Find or start a conversation"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              aria-label="Search peers"
              style={{
                width: "100%",
                padding: "0.5rem 0.75rem 0.5rem 2rem",
                background: "var(--color-bc-surface-1)",
                border: "1px solid rgba(255,255,255,0.1)",
                borderRadius: "4px",
                color: "var(--color-bc-text)",
                fontSize: "0.875rem",
                outline: "none",
                boxSizing: "border-box",
              }}
            />
          </div>
        </div>

        {/* Results */}
        <div style={{ flex: 1, overflowY: "auto", padding: "0.5rem 0.5rem" }}>
          {/* Direct peer ID entry */}
          {directPeerId && (
            <button
              onClick={() => handleSelect(directPeerId)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.75rem",
                width: "100%",
                padding: "0.5rem 0.75rem",
                background: "none",
                border: "none",
                cursor: "pointer",
                borderRadius: "4px",
                textAlign: "left",
                marginBottom: filtered.length > 0 ? "0.25rem" : 0,
              }}
              onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
              onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
            >
              <div
                aria-hidden="true"
                style={{
                  width: "36px",
                  height: "36px",
                  borderRadius: "50%",
                  background: "var(--color-bc-surface-3)",
                  color: "var(--color-bc-muted)",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  flexShrink: 0,
                }}
              >
                <UserPlus size={16} />
              </div>
              <div style={{ flex: 1, overflow: "hidden" }}>
                <div style={{ fontSize: "0.875rem", fontWeight: 500, color: "var(--color-bc-text)" }}>
                  Open DM with peer ID
                </div>
                <div style={{ fontSize: "0.6875rem", color: "var(--color-bc-muted)", fontFamily: "monospace", overflow: "hidden", textOverflow: "ellipsis" }}>
                  {directPeerId.slice(0, 24)}…
                </div>
              </div>
            </button>
          )}
          {filtered.length === 0 && !directPeerId ? (
            <p style={{ color: "var(--color-bc-muted)", fontSize: "0.875rem", textAlign: "center", padding: "1.5rem 1rem", margin: 0 }}>
              {trimmed ? "No peers found. Paste a full 64-character peer ID to message anyone." : "No known peers yet. Join a community to find people."}
            </p>
          ) : (
            filtered.map((peer) => {
              const status = getStatus(peer.user_id);
              const initials = peer.display_name
                .split(/\s+/)
                .map((w) => w[0] ?? "")
                .join("")
                .slice(0, 2)
                .toUpperCase();
              return (
                <button
                  key={peer.user_id}
                  onClick={() => handleSelect(peer.user_id)}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: "0.75rem",
                    width: "100%",
                    padding: "0.5rem 0.75rem",
                    background: "none",
                    border: "none",
                    cursor: "pointer",
                    borderRadius: "4px",
                    textAlign: "left",
                  }}
                  onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
                  onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
                >
                  <div style={{ position: "relative", flexShrink: 0 }}>
                    <div
                      aria-hidden="true"
                      style={{
                        width: "36px",
                        height: "36px",
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
                  <div style={{ flex: 1, overflow: "hidden" }}>
                    <div style={{ fontSize: "0.875rem", fontWeight: 500, color: "var(--color-bc-text)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {peer.display_name}
                    </div>
                    <div style={{ fontSize: "0.6875rem", color: "var(--color-bc-muted)", fontFamily: "monospace", overflow: "hidden", textOverflow: "ellipsis" }}>
                      {peer.user_id.slice(0, 24)}…
                    </div>
                  </div>
                </button>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
