import { useRef, useEffect, useCallback, useMemo, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, Loader2 } from "lucide-react";
import { MessageBubble } from "./MessageBubble";
import type { ExtendedMessage } from "./MessageBubble";

interface ListItem {
  message: ExtendedMessage;
  isGrouped: boolean;
  replyToMessage: ExtendedMessage | null;
  replyToAuthorName: string | undefined;
}

interface MessageListProps {
  messages: ExtendedMessage[];
  myUserId: string;
  communityId: string;
  getMemberName: (userId: string) => string;
  editingMessageId: string | null;
  onStartEdit: (messageId: string, body: string) => void;
  onSaveEdit: (messageId: string, newBody: string) => void;
  onCancelEdit: () => void;
  onReply: (messageId: string) => void;
  onLoadMore: () => Promise<void>;
  hasMore: boolean;
  isLoadingMore: boolean;
  /** When false, message actions (react, edit, delete) are disabled. */
  reachable?: boolean;
}

const GROUP_THRESHOLD_MS = 5 * 60 * 1000; // 5 minutes

export function MessageList({
  messages,
  myUserId,
  communityId,
  getMemberName,
  editingMessageId,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
  onReply,
  onLoadMore,
  hasMore,
  isLoadingMore,
  reachable = true,
}: MessageListProps) {
  const parentRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const prevCountRef = useRef(0);
  const prevLastIdRef = useRef<string | null>(null);
  const loadingMoreRef = useRef(false);
  const [newCount, setNewCount] = useState(0);
  const [showJumpButton, setShowJumpButton] = useState(false);
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const highlightTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Compute grouped items
  const items: ListItem[] = useMemo(() => {
    const msgMap = new Map<string, ExtendedMessage>(
      messages.map((m) => [m.id, m])
    );

    return messages.map((msg, i) => {
      const prev = messages[i - 1];
      const isGrouped =
        !!prev &&
        prev.author_id === msg.author_id &&
        !prev.deleted &&
        !msg.deleted &&
        new Date(msg.timestamp).getTime() - new Date(prev.timestamp).getTime() <
          GROUP_THRESHOLD_MS;

      const replyToMessage = msg.reply_to ? (msgMap.get(msg.reply_to) ?? null) : null;
      const replyToAuthorName = replyToMessage
        ? getMemberName(replyToMessage.author_id)
        : undefined;

      return { message: msg, isGrouped, replyToMessage, replyToAuthorName };
    });
  }, [messages, getMemberName]);

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 56,
    overscan: 20,
    measureElement: (el) => el.getBoundingClientRect().height,
  });

  // Scroll to bottom on channel first load / channel switch
  useEffect(() => {
    // Reset tracking refs immediately so the items.length effect below doesn't
    // miscount the initial batch of messages as "new".
    prevCountRef.current = 0;
    prevLastIdRef.current = null;
    atBottomRef.current = true;
    if (items.length > 0) {
      setTimeout(() => {
        virtualizer.scrollToIndex(items.length - 1, {
          align: "end",
          behavior: "instant",
        });
        setShowJumpButton(false);
        setNewCount(0);
      }, 0);
    }
    // Only on initial load / channel switch — intentionally incomplete deps
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [communityId]);

  // Auto-scroll when new messages arrive (if at bottom)
  useEffect(() => {
    const count = items.length;
    const lastId = items[count - 1]?.message.id ?? null;

    if (count > prevCountRef.current) {
      const added = count - prevCountRef.current;
      // Only treat as "new" if a message was appended at the bottom.
      // When onLoadMore prepends old messages, the last message ID stays the same.
      const appendedAtBottom = lastId !== prevLastIdRef.current;
      if (appendedAtBottom) {
        if (atBottomRef.current) {
          setTimeout(() => {
            virtualizer.scrollToIndex(count - 1, {
              align: "end",
              behavior: "smooth",
            });
          }, 0);
        } else {
          setNewCount((n) => n + added);
          setShowJumpButton(true);
        }
      }
    }

    prevCountRef.current = count;
    prevLastIdRef.current = lastId;
  }, [items.length, virtualizer]);

  // Auto-scroll when the last message grows (e.g. a reaction is added) and user is at the bottom
  const lastItemReactionKey = items.length > 0
    ? (items[items.length - 1].message.reactions ?? []).map((r) => `${r.emoji}:${r.user_ids.length}`).join(",")
    : "";
  useEffect(() => {
    if (!lastItemReactionKey || !atBottomRef.current || items.length === 0) return;
    setTimeout(() => {
      virtualizer.scrollToIndex(items.length - 1, { align: "end", behavior: "smooth" });
    }, 0);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastItemReactionKey]);

  // Track scroll position
  const handleScroll = useCallback(() => {
    const el = parentRef.current;
    if (!el) return;
    const distFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    const wasAtBottom = atBottomRef.current;
    atBottomRef.current = distFromBottom < 120;

    if (atBottomRef.current && !wasAtBottom) {
      setShowJumpButton(false);
      setNewCount(0);
    }

    // Load more when near the top
    if (
      el.scrollTop < 200 &&
      !loadingMoreRef.current &&
      !isLoadingMore
    ) {
      loadingMoreRef.current = true;
      void onLoadMore().finally(() => {
        loadingMoreRef.current = false;
      });
    }
  }, [onLoadMore, isLoadingMore]);

  const handleJumpToBottom = () => {
    virtualizer.scrollToIndex(items.length - 1, {
      align: "end",
      behavior: "smooth",
    });
    setShowJumpButton(false);
    setNewCount(0);
    atBottomRef.current = true;
  };

  const handleScrollToMessage = useCallback((messageId: string) => {
    const idx = items.findIndex((item) => item.message.id === messageId);
    if (idx === -1) return;
    virtualizer.scrollToIndex(idx, { align: "center", behavior: "instant" });
    if (highlightTimerRef.current) clearTimeout(highlightTimerRef.current);
    // Double-RAF: the virtualizer updates its internal scroll state asynchronously
    // via the scroll event. Two frames ensures it has re-rendered with the new
    // virtual items before we apply the highlight, so the target element exists in the DOM.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        setHighlightedId(messageId);
        highlightTimerRef.current = setTimeout(() => setHighlightedId(null), 1000);
      });
    });
  }, [items, virtualizer]);

  if (items.length === 0) {
    return (
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: "0.5rem",
          color: "var(--color-bc-muted)",
          padding: "2rem",
        }}
      >
        <span style={{ fontSize: "2rem" }}>💬</span>
        <p style={{ margin: 0, fontWeight: 600, color: "var(--color-bc-text)" }}>
          No messages yet
        </p>
        <p style={{ margin: 0, fontSize: "0.875rem" }}>
          Be the first to say something!
        </p>
      </div>
    );
  }

  return (
    <div style={{ flex: 1, position: "relative", overflow: "hidden" }}>
      {/* Load more indicator */}
      {isLoadingMore && (
        <div
          style={{
            position: "absolute",
            top: "0.5rem",
            left: "50%",
            transform: "translateX(-50%)",
            background: "var(--color-bc-surface-2)",
            border: "1px solid rgba(255,255,255,0.08)",
            borderRadius: "16px",
            padding: "4px 12px",
            fontSize: "0.8125rem",
            color: "var(--color-bc-muted)",
            display: "flex",
            alignItems: "center",
            gap: "6px",
            zIndex: 10,
          }}
        >
          <Loader2 size={14} style={{ animation: "spin 1s linear infinite" }} />
          Loading older messages…
        </div>
      )}

      {/* Scrollable container */}
      <div
        ref={parentRef}
        onScroll={handleScroll}
        style={{
          height: "100%",
          overflowY: "auto",
          overflowX: "hidden",
        }}
        role="log"
        aria-label="Messages"
        aria-live="polite"
      >
        {/* Start of channel */}
        {!hasMore && (
          <div
            style={{
              padding: "2rem 1rem 1rem",
              color: "var(--color-bc-muted)",
              fontSize: "0.875rem",
              borderBottom: "1px solid rgba(255,255,255,0.04)",
              marginBottom: "1rem",
            }}
          >
            This is the beginning of the channel.
          </div>
        )}

        {/* Virtualizer container */}
        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            width: "100%",
            position: "relative",
          }}
        >
          {virtualizer.getVirtualItems().map((virtualItem) => {
            const item = items[virtualItem.index];
            if (!item) return null;
            return (
              <div
                key={virtualItem.key}
                data-index={virtualItem.index}
                ref={virtualizer.measureElement}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${virtualItem.start}px)`,
                }}
              >
                <MessageBubble
                  message={item.message}
                  isGrouped={item.isGrouped}
                  myUserId={myUserId}
                  communityId={communityId}
                  displayName={getMemberName(item.message.author_id)}
                  replyToMessage={item.replyToMessage}
                  replyToAuthorName={item.replyToAuthorName}
                  isEditing={editingMessageId === item.message.id}
                  isHighlighted={highlightedId === item.message.id}
                  onReply={onReply}
                  onScrollToMessage={handleScrollToMessage}
                  onStartEdit={onStartEdit}
                  onSaveEdit={(newBody) => onSaveEdit(item.message.id, newBody)}
                  onCancelEdit={onCancelEdit}
                  reachable={reachable}
                />
              </div>
            );
          })}
        </div>
      </div>

      {/* Jump to bottom button */}
      {showJumpButton && (
        <button
          onClick={handleJumpToBottom}
          aria-label={newCount > 0 ? `${newCount} new messages — jump to bottom` : "Jump to bottom"}
          style={{
            position: "absolute",
            bottom: "1rem",
            left: "50%",
            transform: "translateX(-50%)",
            background: "var(--color-bc-accent)",
            border: "none",
            borderRadius: "20px",
            color: "#fff",
            cursor: "pointer",
            padding: "0.375rem 0.875rem 0.375rem 0.75rem",
            display: "flex",
            alignItems: "center",
            gap: "6px",
            fontSize: "0.8125rem",
            fontWeight: 600,
            boxShadow: "0 2px 8px rgba(0,0,0,0.4)",
            zIndex: 10,
          }}
        >
          <ChevronDown size={16} />
          {newCount > 0 ? `${newCount} new message${newCount === 1 ? "" : "s"}` : "Jump to bottom"}
        </button>
      )}

      {/* Keyframes */}
      <style>{`
        @keyframes spin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
      `}</style>
    </div>
  );
}
