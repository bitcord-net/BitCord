import React, { useState, useRef, useEffect } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Hash, Volume2, Megaphone, Plus, MoreVertical, Trash2, RotateCw } from "lucide-react";
import type { ChannelInfo, ChannelKind } from "../lib/rpc-types";
import { useCommunitiesStore } from "../store/communities";
import { useMessagesStore } from "../store/messages";
import { useIdentityStore } from "../store/identity";
import { rpcClient } from "../hooks/useRpc";
import { CreateChannelModal } from "./CreateChannelModal";
import { toast } from "../store/toast";

const KIND_ICON: Record<ChannelKind, React.ReactNode> = {
  text: <Hash size={16} aria-hidden="true" />,
  announcement: <Megaphone size={16} aria-hidden="true" />,
  voice: <Volume2 size={16} aria-hidden="true" />,
};

const KIND_LABEL: Record<ChannelKind, string> = {
  text: "Text Channels",
  announcement: "Announcements",
  voice: "Voice Channels",
};

const KIND_ORDER: ChannelKind[] = ["announcement", "text", "voice"];

interface Props {
  communityId: string;
}

export function ChannelList({ communityId }: Props) {
  const navigate = useNavigate();
  const { chid } = useParams();
  const { channels, addChannel, removeChannel, reorderChannels } = useCommunitiesStore();
  const { unreadCounts, clearUnread } = useMessagesStore();
  const { identity } = useIdentityStore();
  const communities = useCommunitiesStore((s) => s.communities);

  const [showCreateModal, setShowCreateModal] = useState(false);
  const [initialKind, setInitialKind] = useState<ChannelKind>("text");
  const [contextMenu, setContextMenu] = useState<{ channelId: string; x: number; y: number } | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const dragRef = useRef<string | null>(null);

  const community = communities.find((c) => c.id === communityId);
  const isAdmin = identity && community ? community.admin_ids.includes(identity.peer_id) : false;
  const isReachable = community?.reachable ?? true;
  const channelList = channels[communityId] ?? [];

  const grouped = KIND_ORDER.reduce<Record<ChannelKind, ChannelInfo[]>>(
    (acc, k) => {
      acc[k] = channelList.filter((ch) => ch.kind === k);
      return acc;
    },
    { text: [], announcement: [], voice: [] }
  );

  const handleSelect = (ch: ChannelInfo) => {
    clearUnread(ch.id);
    navigate(`/app/community/${communityId}/channel/${ch.id}`);
    setContextMenu(null);
  };

  const openCreateModal = (kind: ChannelKind) => {
    setInitialKind(kind);
    setShowCreateModal(true);
  };

  const handleContextMenu = (e: React.MouseEvent, channelId: string) => {
    if (!isAdmin) return;
    e.preventDefault();
    setContextMenu({ channelId, x: e.clientX, y: e.clientY });
  };

  const handleDelete = async (ch: ChannelInfo) => {
    setContextMenu(null);
    try {
      await rpcClient.channelDelete({ community_id: communityId, channel_id: ch.id });
      removeChannel(communityId, ch.id);
    } catch {
      toast("Failed to delete channel.", "error");
    }
  };

  const handleRotateKey = async (ch: ChannelInfo) => {
    setContextMenu(null);
    try {
      await rpcClient.channelRotateKey({ community_id: communityId, channel_id: ch.id });
      toast("Channel key rotated.", "success");
    } catch {
      toast("Failed to rotate channel key.", "error");
    }
  };

  // Drag-to-reorder handlers (admin only)
  const handleDragStart = (channelId: string) => {
    dragRef.current = channelId;
  };

  const handleDragOver = (e: React.DragEvent, channelId: string) => {
    e.preventDefault();
    setDragOver(channelId);
  };

  const handleDrop = (targetKind: ChannelKind, targetId: string) => {
    if (!dragRef.current || dragRef.current === targetId) {
      setDragOver(null);
      return;
    }
    const kindChannels = grouped[targetKind];
    const fromIndex = kindChannels.findIndex((ch) => ch.id === dragRef.current);
    const toIndex = kindChannels.findIndex((ch) => ch.id === targetId);
    if (fromIndex === -1 || toIndex === -1) {
      setDragOver(null);
      return;
    }
    const reordered = [...kindChannels];
    const [moved] = reordered.splice(fromIndex, 1);
    reordered.splice(toIndex, 0, moved);
    // Rebuild full list preserving other kinds' order
    const newOrder = KIND_ORDER.flatMap((k) => (k === targetKind ? reordered : grouped[k]));
    reorderChannels(communityId, newOrder.map((ch) => ch.id));
    dragRef.current = null;
    setDragOver(null);
  };

  return (
    <nav aria-label="Channel list" style={{ flex: 1, overflowY: "auto", padding: "0.5rem 0" }}>
      {KIND_ORDER.map((kind) => {
        const list = grouped[kind];
        if (list.length === 0 && !isAdmin) return null;
        return (
          <div key={kind}>
            {/* Group header */}
            <div
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                padding: "0.625rem 0.75rem 0.25rem",
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
                {KIND_LABEL[kind]}
              </span>
              {isAdmin && (
                <button
                  onClick={() => openCreateModal(kind)}
                  disabled={!isReachable}
                  aria-label={`Create ${kind} channel`}
                  title={isReachable ? `Create ${kind} channel` : "Seed node unreachable"}
                  style={{
                    background: "none",
                    border: "none",
                    color: "var(--color-bc-muted)",
                    cursor: isReachable ? "pointer" : "not-allowed",
                    opacity: isReachable ? 1 : 0.35,
                    padding: "1px",
                    display: "flex",
                    borderRadius: "3px",
                  }}
                  onMouseEnter={(e) => { if (isReachable) (e.currentTarget as HTMLElement).style.color = "var(--color-bc-text)"; }}
                  onMouseLeave={(e) => { if (isReachable) (e.currentTarget as HTMLElement).style.color = "var(--color-bc-muted)"; }}
                >
                  <Plus size={14} />
                </button>
              )}
            </div>

            {/* Channel items */}
            {list.map((ch) => {
              const isActive = ch.id === chid;
              const unread = unreadCounts[ch.id] ?? 0;
              return (
                <div
                  key={ch.id}
                  draggable={isAdmin}
                  onDragStart={() => handleDragStart(ch.id)}
                  onDragOver={(e) => handleDragOver(e, ch.id)}
                  onDrop={() => handleDrop(kind, ch.id)}
                  onDragEnd={() => setDragOver(null)}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    margin: "0 0.5rem",
                    borderRadius: "4px",
                    background: isActive
                      ? "var(--color-bc-surface-hover)"
                      : dragOver === ch.id
                      ? "var(--color-bc-surface-3)"
                      : "transparent",
                    outline: dragOver === ch.id ? "1px solid var(--color-bc-accent)" : "none",
                  }}
                >
                  <button
                    onClick={() => handleSelect(ch)}
                    onContextMenu={(e) => handleContextMenu(e, ch.id)}
                    aria-label={`Channel: ${ch.name}`}
                    aria-current={isActive ? "page" : undefined}
                    style={{
                      flex: 1,
                      display: "flex",
                      alignItems: "center",
                      gap: "0.375rem",
                      padding: "0.375rem 0.75rem",
                      border: "none",
                      background: "transparent",
                      color: isActive
                        ? "var(--color-bc-text)"
                        : unread > 0
                        ? "var(--color-bc-text)"
                        : "var(--color-bc-muted)",
                      cursor: "pointer",
                      fontSize: "0.9375rem",
                      textAlign: "left",
                      fontWeight: unread > 0 ? 600 : 400,
                    }}
                  >
                    <span style={{ flexShrink: 0, color: isActive ? "var(--color-bc-text)" : "var(--color-bc-muted)" }}>
                      {KIND_ICON[ch.kind]}
                    </span>
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>
                      {ch.name}
                    </span>
                    {unread > 0 && !isActive && (
                      <span
                        aria-label={`${unread} unread messages`}
                        style={{
                          background: "var(--color-bc-danger)",
                          color: "#fff",
                          borderRadius: "10px",
                          fontSize: "0.6875rem",
                          fontWeight: 700,
                          padding: "0 5px",
                          minWidth: "18px",
                          textAlign: "center",
                          flexShrink: 0,
                        }}
                      >
                        {unread > 99 ? "99+" : unread}
                      </span>
                    )}
                  </button>

                  {/* Context menu trigger (admin) */}
                  {isAdmin && (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        const rect = e.currentTarget.getBoundingClientRect();
                        setContextMenu({ channelId: ch.id, x: rect.left, y: rect.bottom });
                      }}
                      aria-label={`Options for ${ch.name}`}
                      style={{
                        background: "none",
                        border: "none",
                        color: "var(--color-bc-muted)",
                        cursor: "pointer",
                        padding: "4px",
                        display: "flex",
                        borderRadius: "3px",
                        opacity: isActive ? 1 : 0,
                        transition: "opacity 0.1s",
                        marginRight: "4px",
                      }}
                      onFocus={(e) => ((e.currentTarget as HTMLElement).style.opacity = "1")}
                      onBlur={(e) => ((e.currentTarget as HTMLElement).style.opacity = isActive ? "1" : "0")}
                    >
                      <MoreVertical size={14} aria-hidden="true" />
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        );
      })}

      {/* Context menu */}
      {contextMenu && (
        <ChannelContextMenu
          channel={channelList.find((ch) => ch.id === contextMenu.channelId)!}
          x={contextMenu.x}
          y={contextMenu.y}
          isReachable={isReachable}
          onClose={() => setContextMenu(null)}
          onDelete={handleDelete}
          onRotateKey={handleRotateKey}
        />
      )}

      {/* Create channel modal */}
      {showCreateModal && (
        <CreateChannelModal
          communityId={communityId}
          initialKind={initialKind}
          onClose={() => setShowCreateModal(false)}
          onCreated={(channel) => {
            addChannel(communityId, channel);
            setShowCreateModal(false);
            navigate(`/app/community/${communityId}/channel/${channel.id}`);
          }}
        />
      )}
    </nav>
  );
}

function ChannelContextMenu({
  channel,
  x,
  y,
  isReachable,
  onClose,
  onDelete,
  onRotateKey,
}: {
  channel: ChannelInfo;
  x: number;
  y: number;
  isReachable: boolean;
  onClose: () => void;
  onDelete: (ch: ChannelInfo) => void;
  onRotateKey: (ch: ChannelInfo) => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  useEffect(() => { onCloseRef.current = onClose; });

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) {
        onCloseRef.current();
      }
    };
    // Delay so the mousedown that opened the menu doesn't immediately close it.
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

  if (!channel) return null;

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
        minWidth: "160px",
      }}
    >
      {channel.kind === "text" && (
        <button
          role="menuitem"
          onClick={() => onRotateKey(channel)}
          disabled={!isReachable}
          title={!isReachable ? "Seed node unreachable" : undefined}
          style={{
            ...itemStyle,
            color: "var(--color-bc-text)",
            opacity: !isReachable ? 0.4 : 1,
            cursor: !isReachable ? "not-allowed" : "pointer",
          }}
          onMouseEnter={(e) => { if (isReachable) (e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)"; }}
          onMouseLeave={(e) => { if (isReachable) (e.currentTarget as HTMLElement).style.background = "none"; }}
        >
          <RotateCw size={14} aria-hidden="true" />
          Rotate Key
        </button>
      )}
      <button
        role="menuitem"
        onClick={() => onDelete(channel)}
        disabled={!isReachable}
        title={!isReachable ? "Seed node unreachable" : undefined}
        style={{
          ...itemStyle,
          color: "var(--color-bc-danger)",
          opacity: !isReachable ? 0.4 : 1,
          cursor: !isReachable ? "not-allowed" : "pointer",
        }}
        onMouseEnter={(e) => { if (isReachable) (e.currentTarget as HTMLElement).style.background = "rgba(237,66,69,0.12)"; }}
        onMouseLeave={(e) => { if (isReachable) (e.currentTarget as HTMLElement).style.background = "none"; }}
      >
        <Trash2 size={14} aria-hidden="true" />
        Delete Channel
      </button>
    </div>
  );
}
