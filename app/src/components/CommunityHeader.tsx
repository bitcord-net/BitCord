import React, { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Link2, Settings, ChevronDown, Check } from "lucide-react";
import type { CommunityInfo } from "../lib/rpc-types";
import { useCommunitiesStore } from "../store/communities";
import { useIdentityStore } from "../store/identity";
import { rpcClient } from "../hooks/useRpc";
import { RpcError } from "../lib/rpc-client";

interface Props {
  community: CommunityInfo;
}

function buildInviteLink(community: CommunityInfo): string {
  const payload = JSON.stringify({
    community_id: community.id,
    name: community.name,
    description: community.description,
    public_key_hex: community.public_key_hex,
    seed_nodes: community.seed_nodes,
  });
  // base64url encode
  const b64 = btoa(payload)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  return `bitcord://join/${b64}`;
}

export function CommunityHeader({ community }: Props) {
  const navigate = useNavigate();
  const { identity } = useIdentityStore();
  const { members, removeCommunity } = useCommunitiesStore();
  const [copied, setCopied] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirmMode, setConfirmMode] = useState<"leave" | "delete" | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const isAdmin = identity ? community.admin_ids.includes(identity.peer_id) : false;
  const memberList = members[community.id] ?? [];
  const onlineCount = memberList.filter((m) => m.status === "online" || m.status === "idle").length;
  const totalCount = memberList.length;

  const leaveCommunity = async () => {
    setBusy(true);
    setActionError(null);
    try {
      await rpcClient.communityLeave(community.id);
      removeCommunity(community.id);
      navigate("/app");
    } catch (err) {
      setActionError(err instanceof RpcError ? err.message : "Failed to leave community.");
      setBusy(false);
    }
  };

  const deleteCommunity = async () => {
    setBusy(true);
    setActionError(null);
    try {
      await rpcClient.communityDelete(community.id);
      removeCommunity(community.id);
      navigate("/app");
    } catch (err) {
      setActionError(err instanceof RpcError ? err.message : "Failed to delete community.");
      setBusy(false);
    }
  };

  const copyInvite = async () => {
    let link: string;
    if (isAdmin) {
      // Admins get a cryptographically signed invite from the backend.
      try {
        link = await rpcClient.communityGenerateInvite(community.id);
      } catch {
        // Fall back to unsigned invite if the RPC fails unexpectedly.
        link = buildInviteLink(community);
      }
    } else {
      // Non-admins build an unsigned invite using only the admin-specified seed nodes.
      link = buildInviteLink(community);
    }
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // fallback: create a temporary input
      const el = document.createElement("input");
      el.value = link;
      document.body.appendChild(el);
      el.select();
      document.execCommand("copy");
      document.body.removeChild(el);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
    setMenuOpen(false);
  };

  return (
    <div
      style={{
        padding: "0 1rem",
        height: "48px",
        display: "flex",
        alignItems: "center",
        borderBottom: "1px solid var(--color-bc-surface-1)",
        flexShrink: 0,
        position: "relative",
      }}
    >
      {/* Name + dropdown trigger */}
      <button
        onClick={() => setMenuOpen((o) => !o)}
        aria-label="Community menu"
        aria-expanded={menuOpen}
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          gap: "0.25rem",
          background: "none",
          border: "none",
          cursor: "pointer",
          padding: 0,
          overflow: "hidden",
          textAlign: "left",
        }}
      >
        <span
          style={{
            fontWeight: 700,
            fontSize: "1rem",
            color: "var(--color-bc-text)",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            flex: 1,
          }}
        >
          {community.name}
        </span>
        <ChevronDown size={16} style={{ color: "var(--color-bc-muted)", flexShrink: 0 }} aria-hidden="true" />
      </button>

      {/* Member counts */}
      {totalCount > 0 && (
        <span style={{ fontSize: "0.75rem", color: "var(--color-bc-muted)", marginLeft: "0.5rem", flexShrink: 0 }}>
          {onlineCount}/{totalCount}
        </span>
      )}

      {/* Invite + Settings */}
      <button
        onClick={() => void copyInvite()}
        disabled={isAdmin && !community.reachable}
        title={copied ? "Copied!" : isAdmin && !community.reachable ? "Seed node unreachable" : "Copy Invite Link"}
        aria-label="Copy invite link"
        style={{
          background: "none",
          border: "none",
          color: copied ? "var(--color-bc-success)" : "var(--color-bc-muted)",
          cursor: isAdmin && !community.reachable ? "not-allowed" : "pointer",
          opacity: isAdmin && !community.reachable ? 0.35 : 1,
          padding: "4px",
          display: "flex",
          marginLeft: "0.25rem",
          borderRadius: "3px",
          transition: "color 0.15s",
        }}
      >
        {copied ? <Check size={16} /> : <Link2 size={16} />}
      </button>

      {isAdmin && (
        <button
          onClick={() => navigate(`/app/community/${community.id}/settings`)}
          title="Community Settings"
          aria-label="Community settings"
          style={{
            background: "none",
            border: "none",
            color: "var(--color-bc-muted)",
            cursor: "pointer",
            padding: "4px",
            display: "flex",
            borderRadius: "3px",
            transition: "color 0.15s",
          }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.color = "var(--color-bc-text)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.color = "var(--color-bc-muted)")}
        >
          <Settings size={16} aria-hidden="true" />
        </button>
      )}

      {/* Dropdown menu */}
      {menuOpen && (
        <DropdownMenu
          community={community}
          isAdmin={isAdmin}
          busy={busy}
          onCopyInvite={() => void copyInvite()}
          onNavigateSettings={() => {
            setMenuOpen(false);
            navigate(`/app/community/${community.id}/settings`);
          }}
          confirmMode={confirmMode}
          onRequestLeave={() => { setConfirmMode("leave"); setActionError(null); }}
          onRequestDelete={() => { setConfirmMode("delete"); setActionError(null); }}
          onConfirm={() => { void (confirmMode === "delete" ? deleteCommunity() : leaveCommunity()); }}
          onCancelConfirm={() => { setConfirmMode(null); setActionError(null); }}
          actionError={actionError}
          onClose={() => { setMenuOpen(false); setConfirmMode(null); setActionError(null); }}
        />
      )}
    </div>
  );
}

function DropdownMenu({
  community,
  isAdmin,
  busy,
  confirmMode,
  actionError,
  onCopyInvite,
  onNavigateSettings,
  onRequestLeave,
  onRequestDelete,
  onConfirm,
  onCancelConfirm,
  onClose,
}: {
  community: CommunityInfo;
  isAdmin: boolean;
  busy: boolean;
  confirmMode: "leave" | "delete" | null;
  actionError: string | null;
  onCopyInvite: () => void;
  onNavigateSettings: () => void;
  onRequestLeave: () => void;
  onRequestDelete: () => void;
  onConfirm: () => void;
  onCancelConfirm: () => void;
  onClose: () => void;
}) {
  // Close on outside click
  const ref = (el: HTMLDivElement | null) => {
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
    color: "var(--color-bc-text)",
    cursor: "pointer",
    fontSize: "0.9375rem",
    textAlign: "left",
    borderRadius: "3px",
  };

  if (confirmMode) {
    const isDelete = confirmMode === "delete";
    return (
      <div
        ref={ref}
        role="dialog"
        style={{
          position: "absolute",
          top: "46px",
          left: 0,
          right: 0,
          background: "var(--color-bc-surface-1)",
          borderRadius: "4px",
          boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
          zIndex: 100,
          padding: "0.75rem",
          border: "1px solid rgba(255,255,255,0.06)",
        }}
      >
        <p style={{ margin: "0 0 0.5rem", fontSize: "0.875rem", color: "var(--color-bc-text)" }}>
          {isDelete
            ? `Permanently delete "${community.name}"? This cannot be undone.`
            : `Leave "${community.name}"? You'll need a new invite to rejoin.`}
        </p>
        {actionError && (
          <p style={{ margin: "0 0 0.5rem", fontSize: "0.8125rem", color: "var(--color-bc-danger, #ed4245)" }}>
            {actionError}
          </p>
        )}
        <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end" }}>
          <button
            onClick={onCancelConfirm}
            disabled={busy}
            style={{ ...itemStyle, width: "auto", padding: "0.375rem 0.75rem", fontSize: "0.875rem" }}
            onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
            onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={busy}
            style={{
              ...itemStyle,
              width: "auto",
              padding: "0.375rem 0.75rem",
              fontSize: "0.875rem",
              color: "var(--color-bc-danger, #ed4245)",
            }}
            onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "rgba(237,66,69,0.15)")}
            onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
          >
            {busy ? (isDelete ? "Deleting…" : "Leaving…") : (isDelete ? "Delete" : "Leave")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div
      ref={ref}
      role="menu"
      style={{
        position: "absolute",
        top: "46px",
        left: 0,
        right: 0,
        background: "var(--color-bc-surface-1)",
        borderRadius: "4px",
        boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
        zIndex: 100,
        padding: "0.25rem",
        border: "1px solid rgba(255,255,255,0.06)",
      }}
    >
      <button
        role="menuitem"
        onClick={onCopyInvite}
        disabled={isAdmin && !community.reachable}
        title={isAdmin && !community.reachable ? "Seed node unreachable" : undefined}
        style={{
          ...itemStyle,
          opacity: isAdmin && !community.reachable ? 0.4 : 1,
          cursor: isAdmin && !community.reachable ? "not-allowed" : "pointer",
        }}
        onMouseEnter={(e) => { if (!(isAdmin && !community.reachable)) (e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)"; }}
        onMouseLeave={(e) => { if (!(isAdmin && !community.reachable)) (e.currentTarget as HTMLElement).style.background = "none"; }}
      >
        Copy Invite Link
      </button>
      {isAdmin && (
        <button
          role="menuitem"
          onClick={onNavigateSettings}
          style={itemStyle}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
        >
          Community Settings
        </button>
      )}
      <div style={{ height: "1px", background: "rgba(255,255,255,0.06)", margin: "0.25rem 0" }} />
      {isAdmin ? (
        <button
          role="menuitem"
          onClick={onRequestDelete}
          style={{ ...itemStyle, color: "var(--color-bc-danger, #ed4245)" }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "rgba(237,66,69,0.15)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
        >
          Delete Community
        </button>
      ) : (
        <button
          role="menuitem"
          onClick={onRequestLeave}
          style={{ ...itemStyle, color: "var(--color-bc-danger, #ed4245)" }}
          onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "rgba(237,66,69,0.15)")}
          onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = "none")}
        >
          Leave Community
        </button>
      )}
    </div>
  );
}
