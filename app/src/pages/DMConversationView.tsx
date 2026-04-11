import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { useParams } from "react-router-dom";
import { useIsMobile } from "../hooks/useIsMobile";
import { Loader2, Lock, Smile, Reply, Edit2, Copy } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { formatDistanceToNow, format, isToday, isYesterday } from "date-fns";
import { useIdentityStore } from "../store/identity";
import { useCommunitiesStore } from "../store/communities";
import { usePresenceStore } from "../store/presence";
import { useDmsStore } from "../store/dms";
import type { DmReactionGroup } from "../store/dms";
import { useSubscription } from "../hooks/useSubscription";
import { rpcClient } from "../hooks/useRpc";
import { MessageInput } from "../components/MessageInput";
import { PresenceIndicator } from "../components/PresenceIndicator";
import { EmojiPicker, ActionButton } from "../components/MessageBubble";
import { statusLabel } from "../lib/statusHelpers";
import type { DmMessageInfo } from "../lib/rpc-types";

const HISTORY_LIMIT = 50;

function formatTimestamp(ts: string): string {
  const d = new Date(ts);
  if (isToday(d)) return `Today at ${format(d, "h:mm a")}`;
  if (isYesterday(d)) return `Yesterday at ${format(d, "h:mm a")}`;
  return format(d, "MMM d, yyyy h:mm a");
}

// ── DM Reaction bar ───────────────────────────────────────────────────────────

function DmReactionBar({
  reactions,
  myUserId,
  onToggle,
}: {
  reactions: DmReactionGroup[];
  myUserId: string;
  onToggle: (emoji: string) => void;
}) {
  if (reactions.length === 0) return null;
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "4px", marginTop: "4px" }}>
      {reactions.map(({ emoji, userIds }) => {
        const hasReacted = userIds.includes(myUserId);
        return (
          <button
            key={emoji}
            onClick={() => onToggle(emoji)}
            title={`${userIds.length} ${userIds.length === 1 ? "person" : "people"} reacted`}
            style={{
              border: `1px solid ${hasReacted ? "var(--color-bc-accent)" : "rgba(255,255,255,0.1)"}`,
              background: hasReacted ? "rgba(88,101,242,0.2)" : "var(--color-bc-surface-2)",
              borderRadius: "12px",
              padding: "2px 8px",
              cursor: "pointer",
              display: "flex",
              alignItems: "center",
              gap: "4px",
              fontSize: "0.8125rem",
              color: "var(--color-bc-text)",
            }}
          >
            <span>{emoji}</span>
            <span style={{ fontWeight: 600 }}>{userIds.length}</span>
          </button>
        );
      })}
    </div>
  );
}

// ── DmMessageBubble ───────────────────────────────────────────────────────────

function DmMessageBubble({
  msg,
  isOwn,
  authorName,
  isGrouped,
  isEditing,
  isHighlighted,
  reactions,
  myUserId,
  replyToMessage,
  replyToAuthorName,
  onReply,
  onScrollToMessage,
  onAddReaction,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
}: {
  msg: DmMessageInfo & { _status?: "pending" | "failed"; _onRetry?: () => void };
  isOwn: boolean;
  authorName: string;
  isGrouped: boolean;
  isEditing: boolean;
  isHighlighted?: boolean;
  reactions: DmReactionGroup[];
  myUserId: string;
  replyToMessage?: (DmMessageInfo & { _status?: "pending" | "failed" }) | null;
  replyToAuthorName?: string;
  onReply: (id: string) => void;
  onScrollToMessage?: (id: string) => void;
  onAddReaction: (emoji: string) => void;
  onStartEdit: (id: string, body: string) => void;
  onSaveEdit: (newBody: string) => void;
  onCancelEdit: () => void;
}) {
  const [editDraft, setEditDraft] = useState(msg.body);
  const [hovered, setHovered] = useState(false);
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const editInputRef = useRef<HTMLTextAreaElement>(null);
  const emojiPickerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (isEditing) {
      setEditDraft(msg.body);
      editInputRef.current?.focus();
    }
  }, [isEditing, msg.body]);

  useEffect(() => {
    if (!showEmojiPicker) return;
    const handler = (e: MouseEvent) => {
      if (emojiPickerRef.current && !emojiPickerRef.current.contains(e.target as Node)) {
        setShowEmojiPicker(false);
        setHovered(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [showEmojiPicker]);

  const initials = authorName
    .split(/\s+/)
    .map((w) => w[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();

  const isPending = msg._status === "pending";
  const isFailed = msg._status === "failed";

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => {
        if (!showEmojiPicker) setHovered(false);
      }}
      style={{
        display: "flex",
        gap: "0.625rem",
        padding: isGrouped ? "1px 1rem 2px" : "0.5rem 1rem 2px",
        opacity: isPending || isFailed ? 0.7 : 1,
        background: hovered ? "rgba(255,255,255,0.02)" : "transparent",
        transition: "background 0.05s",
        position: "relative",
      }}
    >
      {isHighlighted && (
        <div
          aria-hidden="true"
          style={{
            position: "absolute",
            inset: 0,
            animation: "bc-msg-flash 1s ease-out forwards",
            pointerEvents: "none",
          }}
        />
      )}
      {/* Avatar column */}
      <div style={{ width: "36px", flexShrink: 0 }}>
        {!isGrouped && (
          <div
            aria-hidden="true"
            style={{
              width: "36px",
              height: "36px",
              borderRadius: "50%",
              background: isOwn ? "var(--color-bc-accent)" : "var(--color-bc-surface-3)",
              color: "#fff",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 700,
              fontSize: "0.75rem",
              marginTop: "2px",
            }}
          >
            {initials}
          </div>
        )}
      </div>

      {/* Content */}
      <div style={{ flex: 1, overflow: "hidden" }}>
        {!isGrouped && (
          <div
            style={{
              display: "flex",
              alignItems: "baseline",
              gap: "0.5rem",
              marginBottom: "2px",
            }}
          >
            <span
              style={{
                fontWeight: 600,
                fontSize: "0.9375rem",
                color: isOwn ? "var(--color-bc-accent)" : "var(--color-bc-text)",
              }}
            >
              {authorName}
            </span>
            <span
              title={formatTimestamp(msg.timestamp)}
              style={{ fontSize: "0.6875rem", color: "var(--color-bc-muted)" }}
            >
              {formatDistanceToNow(new Date(msg.timestamp), { addSuffix: true })}
            </span>
            {msg.edited_at && (
              <span style={{ fontSize: "0.6875rem", color: "var(--color-bc-muted)" }}>(edited)</span>
            )}
          </div>
        )}

        {/* Reply quote */}
        {replyToMessage && (
          <div
            onClick={() => onScrollToMessage?.(replyToMessage.id)}
            style={{
              display: "flex",
              alignItems: "flex-start",
              gap: "0.375rem",
              marginBottom: "4px",
              fontSize: "0.875rem",
              color: "var(--color-bc-muted)",
              maxWidth: "100%",
              cursor: onScrollToMessage ? "pointer" : undefined,
              borderRadius: "4px",
              padding: "1px 4px 1px 0",
            }}
          >
            <div style={{ width: "2px", flexShrink: 0, alignSelf: "stretch", background: "var(--color-bc-muted)", borderRadius: "1px", opacity: 0.5 }} />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              <strong style={{ color: "var(--color-bc-text)" }}>
                {replyToAuthorName ?? replyToMessage.author_id.slice(0, 8)}
              </strong>{" "}
              {replyToMessage.body.slice(0, 80)}
              {replyToMessage.body.length > 80 ? "…" : ""}
            </span>
          </div>
        )}

        {isEditing ? (
          <div>
            <textarea
              ref={editInputRef}
              value={editDraft}
              onChange={(e) => setEditDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  if (editDraft.trim()) onSaveEdit(editDraft.trim());
                } else if (e.key === "Escape") {
                  onCancelEdit();
                }
              }}
              rows={2}
              style={{
                width: "100%",
                background: "var(--color-bc-surface-1)",
                border: "1px solid var(--color-bc-accent)",
                borderRadius: "4px",
                color: "var(--color-bc-text)",
                fontSize: "0.9375rem",
                padding: "0.375rem 0.5rem",
                resize: "none",
                outline: "none",
                fontFamily: "inherit",
                boxSizing: "border-box",
              }}
            />
            <div style={{ fontSize: "0.75rem", color: "var(--color-bc-muted)", marginTop: "2px" }}>
              Enter to save · Esc to cancel
            </div>
          </div>
        ) : (
          <div
            style={{
              fontSize: "0.9375rem",
              color: "var(--color-bc-text)",
              lineHeight: 1.5,
              wordBreak: "break-word",
            }}
            className="markdown-body"
          >
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{msg.body}</ReactMarkdown>
          </div>
        )}

        {isFailed && (
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem", marginTop: "2px" }}>
            <span style={{ fontSize: "0.75rem", color: "var(--color-bc-danger)" }}>
              {msg._onRetry ? "Failed to send." : "Not delivered — peer may be offline."}
            </span>
            {msg._onRetry && (
              <button
                onClick={msg._onRetry}
                style={{ fontSize: "0.75rem", color: "var(--color-bc-accent)", background: "none", border: "none", cursor: "pointer", padding: 0 }}
              >
                Retry
              </button>
            )}
          </div>
        )}

        <DmReactionBar
          reactions={reactions}
          myUserId={myUserId}
          onToggle={(emoji) => onAddReaction(emoji)}
        />
      </div>

      {/* Hover action bar */}
      {hovered && !isEditing && msg._status !== "pending" && (
        <div
          style={{
            position: "absolute",
            top: "-18px",
            right: "1rem",
            display: "flex",
            gap: "1px",
            background: "var(--color-bc-surface-2)",
            border: "1px solid rgba(255,255,255,0.08)",
            borderRadius: "6px",
            padding: "2px",
            zIndex: 100,
            boxShadow: "0 2px 8px rgba(0,0,0,0.4)",
          }}
          role="toolbar"
          aria-label="Message actions"
        >
          {/* Emoji reaction */}
          <div ref={emojiPickerRef} style={{ position: "relative" }}>
            <ActionButton
              icon={<Smile size={16} />}
              title="Add reaction"
              onClick={() => setShowEmojiPicker((v) => !v)}
            />
            {showEmojiPicker && (
              <EmojiPicker
                onSelect={(emoji) => {
                  onAddReaction(emoji);
                  setShowEmojiPicker(false);
                }}
                onClose={() => setShowEmojiPicker(false)}
              />
            )}
          </div>

          {/* Reply */}
          <ActionButton
            icon={<Reply size={16} />}
            title="Reply"
            onClick={() => {
              onReply(msg.id);
              setHovered(false);
            }}
          />

          {/* Edit (own messages only) */}
          {isOwn && (
            <ActionButton
              icon={<Edit2 size={16} />}
              title="Edit message"
              onClick={() => {
                setEditDraft(msg.body);
                onStartEdit(msg.id, msg.body);
                setHovered(false);
              }}
            />
          )}

          {/* Copy */}
          <ActionButton
            icon={<Copy size={16} />}
            title="Copy text"
            onClick={() => void navigator.clipboard.writeText(msg.body)}
          />
        </div>
      )}
    </div>
  );
}

// ── DMConversationView ────────────────────────────────────────────────────────

type PendingDm = DmMessageInfo & { _status: "pending" | "failed"; _onRetry: () => void };

export function DMConversationView() {
  const { peerId } = useParams<{ peerId: string }>();
  const { identity } = useIdentityStore();
  const { members: allMembers } = useCommunitiesStore();
  const { getStatus } = usePresenceStore();
  const {
    conversations,
    messages: dmMessages,
    localReactions,
    setHistory,
    appendMessage,
    upsertConversation,
    clearUnread,
    toggleReaction,
  } = useDmsStore();

  const [isInitialLoading, setIsInitialLoading] = useState(true);
  const [cachedPeerName, setCachedPeerName] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [replyToId, setReplyToId] = useState<string | null>(null);
  const [pendingMessages, setPendingMessages] = useState<PendingDm[]>([]);
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const [failedMessageIds, setFailedMessageIds] = useState<Set<string>>(new Set());
  const highlightTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const loadedPeerRef = useRef<string | null>(null);
  const isAtBottomRef = useRef(true);

  // Resolve peer display name: community members → stored conversation → DHT cache → truncated ID
  const isMobile = useIsMobile();
  const peerDisplayName = useMemo(() => {
    if (!peerId) return "Unknown";
    for (const memberList of Object.values(allMembers)) {
      const found = memberList.find((m) => m.user_id === peerId);
      if (found) return found.display_name;
    }
    const storedConv = conversations.find((c) => c.peerId === peerId);
    if (storedConv?.displayName && !storedConv.displayName.endsWith("…")) {
      return storedConv.displayName;
    }
    if (cachedPeerName) return cachedPeerName;
    return peerId.slice(0, 12) + "…";
  }, [peerId, allMembers, conversations, cachedPeerName]);

  // Fetch peer display name from DHT cache when not found in community members
  useEffect(() => {
    if (!peerId) return;
    setCachedPeerName(null);
    void rpcClient.dmPeerName(peerId).then((name) => {
      if (name) setCachedPeerName(name);
    });
  }, [peerId]);

  const peerStatus = peerId ? getStatus(peerId) : "offline";
  const myId = identity?.peer_id ?? "";

  const msgs = useMemo(
    () => (peerId ? (dmMessages[peerId] ?? []) : []),
    [peerId, dmMessages],
  );

  // Combined with pending
  const allMsgs: Array<DmMessageInfo & { _status?: "pending" | "failed"; _onRetry?: () => void }> =
    useMemo(() => {
      const storeIds = new Set(msgs.map((m) => m.id));
      const filteredPending = pendingMessages.filter((p) => !storeIds.has(p.id));
      return [...msgs, ...filteredPending];
    }, [msgs, pendingMessages]);

  // Grouping: same author within 5 min
  const groupedMsgs = useMemo(() => {
    const msgMap = new Map(allMsgs.map((m) => [m.id, m]));
    return allMsgs.map((msg, i) => {
      const prev = allMsgs[i - 1];
      const isGrouped =
        !!prev &&
        prev.author_id === msg.author_id &&
        new Date(msg.timestamp).getTime() - new Date(prev.timestamp).getTime() < 5 * 60 * 1000;
      const replyMsg = msg.reply_to ? (msgMap.get(msg.reply_to) ?? null) : null;
      return { msg, isGrouped, replyMsg };
    });
  }, [allMsgs]);

  // Resolve reply-to message and author name
  const replyToMessage = useMemo(
    () => (replyToId ? allMsgs.find((m) => m.id === replyToId) ?? null : null),
    [replyToId, allMsgs],
  );
  const replyToAuthorName = replyToMessage
    ? replyToMessage.author_id === myId
      ? (identity?.display_name ?? "You")
      : peerDisplayName
    : undefined;

  // Load history when peer changes
  useEffect(() => {
    if (!peerId) return;
    if (loadedPeerRef.current === peerId) return;
    loadedPeerRef.current = peerId;

    setIsInitialLoading(true);
    setPendingMessages([]);
    setEditingMessageId(null);
    setReplyToId(null);

    void rpcClient
      .dmGetHistory({ peer_id: peerId, before: null, limit: HISTORY_LIMIT })
      .then((hist) => {
        setHistory(peerId, hist);
        setHasMore(hist.length === HISTORY_LIMIT);
        clearUnread(peerId);
        upsertConversation(peerId, peerDisplayName, hist[hist.length - 1] ?? undefined);
      })
      .catch(() => {
        setHistory(peerId, []);
        setHasMore(false);
      })
      .finally(() => setIsInitialLoading(false));
  }, [peerId, peerDisplayName, setHistory, clearUnread, upsertConversation]);

  // Clear failed message tracking when switching conversations
  useEffect(() => {
    setFailedMessageIds(new Set());
  }, [peerId]);

  // Subscribe to incoming DMs
  useSubscription("dm_new", (ev) => {
    const msg = ev.data.message;
    const convPeerId = msg.author_id === myId ? msg.peer_id : msg.author_id;
    if (convPeerId === peerId) {
      appendMessage(peerId!, msg);
      clearUnread(peerId!);
    } else {
      appendMessage(convPeerId, msg);
      useDmsStore.getState().incrementUnread(convPeerId);
    }
  });

  // Subscribe to DM delivery failures — flag the message this session and discard
  // from the backend so it's absent from dm_get_history on next load (ephemeral).
  useSubscription("dm_send_failed", (ev) => {
    if (ev.data.peer_id === peerId) {
      setFailedMessageIds((prev) => new Set(prev).add(ev.data.message_id));
      void rpcClient.dmDiscard(ev.data.peer_id, ev.data.message_id);
    }
  });

  // Scroll to bottom when switching conversations or after initial load completes
  useEffect(() => {
    isAtBottomRef.current = true;
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [peerId]);

  useEffect(() => {
    if (!isInitialLoading && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [isInitialLoading]);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    if (isAtBottomRef.current && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [allMsgs.length]);

  const handleScroll = () => {
    if (!listRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = listRef.current;
    isAtBottomRef.current = scrollHeight - scrollTop - clientHeight < 80;
  };

  // Load older messages
  const handleLoadMore = useCallback(async () => {
    if (!peerId || !hasMore || isLoadingMore) return;
    const oldest = msgs[0];
    if (!oldest) return;
    setIsLoadingMore(true);
    try {
      const older = await rpcClient.dmGetHistory({ peer_id: peerId, before: oldest.id, limit: HISTORY_LIMIT });
      if (older.length > 0) {
        setHistory(peerId, [...older, ...msgs]);
        setHasMore(older.length === HISTORY_LIMIT);
      } else {
        setHasMore(false);
      }
    } catch { /* keep state */ }
    finally { setIsLoadingMore(false); }
  }, [peerId, hasMore, isLoadingMore, msgs, setHistory]);

  // Scroll-to-top triggers load more
  const handleScrollTop = useCallback(() => {
    if (!listRef.current) return;
    if (listRef.current.scrollTop === 0 && hasMore) {
      void handleLoadMore();
    }
  }, [hasMore, handleLoadMore]);

  // Jump to a specific message by ID and briefly highlight it
  const handleScrollToMessage = useCallback((messageId: string) => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-msg-id="${messageId}"]`);
    if (!el) return;
    el.scrollIntoView({ behavior: "instant", block: "center" });
    if (highlightTimerRef.current) clearTimeout(highlightTimerRef.current);
    setHighlightedId(messageId);
    highlightTimerRef.current = setTimeout(() => setHighlightedId(null), 1000);
  }, []);

  // Send DM
  const handleSend = useCallback(
    async (body: string, replyToMessageId?: string) => {
      if (!peerId) return;
      const tempId = `pending-${Date.now()}-${Math.random()}`;
      const now = new Date().toISOString();

      const doSend = async (id: string) => {
        setPendingMessages((prev) => prev.filter((p) => p.id !== id));
        const pending: PendingDm = {
          id: tempId,
          peer_id: peerId,
          author_id: myId,
          timestamp: now,
          body,
          reply_to: replyToMessageId ?? null,
          edited_at: null,
          _status: "pending",
          _onRetry: () => void doSend(tempId),
        };
        setPendingMessages((prev) => [...prev.filter((p) => p.id !== tempId), pending]);
        try {
          const result = await rpcClient.dmSend({ peer_id: peerId, body, reply_to: replyToMessageId ?? null });
          appendMessage(peerId, result);
          upsertConversation(peerId, peerDisplayName, result);
          setPendingMessages((prev) => prev.filter((p) => p.id !== tempId));
        } catch {
          setPendingMessages((prev) =>
            prev.map((p) => (p.id === tempId ? { ...p, _status: "failed" } : p))
          );
        }
      };
      setReplyToId(null);
      await doSend(tempId);
    },
    [peerId, myId, appendMessage, upsertConversation, peerDisplayName]
  );

  // Edit DM
  const handleSaveEdit = useCallback(
    async (msgId: string, newBody: string) => {
      if (!peerId) return;
      setEditingMessageId(null);
      try {
        // No dm_edit RPC in spec — update optimistically
        useDmsStore.getState().updateMessage(peerId, msgId, { body: newBody, edited_at: new Date().toISOString() });
      } catch { /* ignore */ }
    },
    [peerId]
  );

  if (!peerId) {
    return (
      <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--color-bc-muted)" }}>
        Select a conversation.
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", overflow: "hidden" }}>
      <style>{`@keyframes spin { from{transform:rotate(0deg)} to{transform:rotate(360deg)} }`}</style>
      {/* Header — hidden on mobile (MobileChannelHeader handles navigation) */}
      {!isMobile && <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.625rem",
          padding: "0 1rem",
          height: "48px",
          borderBottom: "1px solid var(--color-bc-surface-1)",
          flexShrink: 0,
          background: "var(--color-bc-surface-3)",
        }}
      >
        <div style={{ position: "relative" }}>
          <div
            aria-hidden="true"
            style={{
              width: "28px",
              height: "28px",
              borderRadius: "50%",
              background: "var(--color-bc-surface-1)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 700,
              fontSize: "0.6875rem",
              color: "var(--color-bc-text)",
            }}
          >
            {peerDisplayName.slice(0, 2).toUpperCase()}
          </div>
          <div style={{ position: "absolute", bottom: 0, right: 0 }}>
            <PresenceIndicator status={peerStatus} size={8} borderColor="var(--color-bc-surface-3)" />
          </div>
        </div>
        <span style={{ fontWeight: 700, fontSize: "0.9375rem", color: "var(--color-bc-text)" }}>
          {peerDisplayName}
        </span>
        <span style={{ fontSize: "0.75rem", color: "var(--color-bc-muted)" }}>
          {statusLabel(peerStatus)}
        </span>
        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: "4px" }}>
          <Lock size={12} aria-hidden="true" style={{ color: "var(--color-bc-muted)" }} />
          <span style={{ fontSize: "0.6875rem", color: "var(--color-bc-muted)" }}>End-to-End Encrypted</span>
        </div>
      </div>}

      {/* Message list */}
      {isInitialLoading ? (
        <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--color-bc-muted)", flexDirection: "column", gap: "0.5rem" }}>
          <Loader2 size={24} style={{ animation: "spin 1s linear infinite" }} />
          <span>Loading messages…</span>
        </div>
      ) : (
        <div
          ref={listRef}
          onScroll={(e) => { handleScroll(); handleScrollTop(); void e; }}
          style={{ flex: 1, overflowY: "auto", display: "flex", flexDirection: "column" }}
        >
          {/* Load more */}
          {hasMore && (
            <div style={{ textAlign: "center", padding: "0.5rem" }}>
              <button
                onClick={() => void handleLoadMore()}
                disabled={isLoadingMore}
                style={{ background: "none", border: "none", color: "var(--color-bc-accent)", cursor: "pointer", fontSize: "0.8125rem" }}
              >
                {isLoadingMore ? "Loading…" : "Load older messages"}
              </button>
            </div>
          )}

          {/* Start of conversation */}
          {!hasMore && (
            <div style={{ padding: "2rem 1rem 1rem", textAlign: "center" }}>
              <div
                aria-hidden="true"
                style={{
                  width: "64px",
                  height: "64px",
                  borderRadius: "50%",
                  background: "var(--color-bc-surface-1)",
                  color: "var(--color-bc-text)",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  fontWeight: 700,
                  fontSize: "1.25rem",
                  margin: "0 auto 0.75rem",
                }}
              >
                {peerDisplayName.slice(0, 2).toUpperCase()}
              </div>
              <div style={{ fontWeight: 700, fontSize: "1.25rem", color: "var(--color-bc-text)", marginBottom: "0.25rem" }}>
                {peerDisplayName}
              </div>
              <div style={{ fontSize: "0.875rem", color: "var(--color-bc-muted)" }}>
                This is the beginning of your direct message history.
              </div>
            </div>
          )}

          {/* Messages */}
          {groupedMsgs.map(({ msg, isGrouped, replyMsg }) => (
            <div key={msg.id} data-msg-id={msg.id}>
              <DmMessageBubble
                msg={failedMessageIds.has(msg.id) ? { ...msg, _status: "failed" } : msg}
                isOwn={msg.author_id === myId}
                authorName={
                  msg.author_id === myId
                    ? (identity?.display_name ?? "You")
                    : peerDisplayName
                }
                isGrouped={isGrouped}
                isEditing={editingMessageId === msg.id}
                isHighlighted={highlightedId === msg.id}
                reactions={localReactions[msg.id] ?? []}
                myUserId={myId}
                replyToMessage={replyMsg}
                replyToAuthorName={
                  replyMsg
                    ? replyMsg.author_id === myId
                      ? (identity?.display_name ?? "You")
                      : peerDisplayName
                    : undefined
                }
                onReply={(id) => setReplyToId(id)}
                onScrollToMessage={handleScrollToMessage}
                onAddReaction={(emoji) => toggleReaction(msg.id, emoji, myId)}
                onStartEdit={(id, body) => setEditingMessageId(body ? id : null)}
                onSaveEdit={(newBody) => void handleSaveEdit(msg.id, newBody)}
                onCancelEdit={() => setEditingMessageId(null)}
              />
            </div>
          ))}
        </div>
      )}

      <div style={{ height: "15px", flexShrink: 0 }} />

      {/* Input */}
      <MessageInput
        channelName={peerDisplayName}
        replyToMessage={replyToMessage ?? null}
        replyToAuthorName={replyToAuthorName}
        onClearReply={() => setReplyToId(null)}
        onSend={handleSend}
        members={[]}
        channels={[]}
        disabled={isInitialLoading}
      />
    </div>
  );
}
