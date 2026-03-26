import { useState, useRef, useEffect } from "react";
import type { ReactNode, KeyboardEvent } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { format, formatDistanceToNow } from "date-fns";
import { Smile, Reply, Edit2, Trash2, Copy } from "lucide-react";
import type { MessageInfo, ReactionInfo } from "../lib/rpc-types";
import { rpcClient } from "../hooks/useRpc";
import { toast } from "../store/toast";

// ── Types ─────────────────────────────────────────────────────────────────────

export type ExtendedMessage = MessageInfo & {
  _status?: "pending" | "failed";
  _onRetry?: () => void;
};

// ── Emoji picker ──────────────────────────────────────────────────────────────

const COMMON_EMOJIS = [
  "👍", "❤️", "😂", "😮", "😢", "😡",
  "🎉", "🔥", "👀", "✅", "🤔", "👏",
  "🙏", "💯", "😊", "🚀", "💪", "🤣",
];

interface EmojiPickerProps {
  onSelect: (emoji: string) => void;
  onClose: () => void;
}

export function EmojiPicker({ onSelect, onClose }: EmojiPickerProps) {
  const pickerRef = useRef<HTMLDivElement>(null);
  const [flipBelow, setFlipBelow] = useState(false);

  // Detect if the picker would be clipped at the top and flip below if so.
  useState(() => {
    // Defer measurement to after the first paint via a microtask.
    queueMicrotask(() => {
      if (pickerRef.current) {
        const rect = pickerRef.current.getBoundingClientRect();
        if (rect.top < 0) {
          setFlipBelow(true);
        }
      }
    });
  });

  return (
    <div
      ref={pickerRef}
      role="dialog"
      aria-label="Emoji picker"
      style={{
        position: "absolute",
        ...(flipBelow
          ? { top: "calc(100% + 4px)" }
          : { bottom: "calc(100% + 4px)" }),
        right: 0,
        background: "var(--color-bc-surface-2)",
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: "8px",
        padding: "0.5rem",
        display: "flex",
        flexWrap: "wrap",
        gap: "2px",
        width: "196px",
        zIndex: 200,
        boxShadow: "0 4px 16px rgba(0,0,0,0.5)",
      }}
    >
      {COMMON_EMOJIS.map((emoji) => (
        <button
          key={emoji}
          onClick={() => {
            onSelect(emoji);
            onClose();
          }}
          aria-label={`React with ${emoji}`}
          style={{
            border: "none",
            background: "transparent",
            cursor: "pointer",
            fontSize: "1.25rem",
            padding: "4px",
            borderRadius: "4px",
            lineHeight: 1,
            width: "36px",
            height: "36px",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
          onMouseEnter={(e) =>
            ((e.currentTarget as HTMLButtonElement).style.background =
              "var(--color-bc-surface-3)")
          }
          onMouseLeave={(e) =>
            ((e.currentTarget as HTMLButtonElement).style.background =
              "transparent")
          }
        >
          {emoji}
        </button>
      ))}
    </div>
  );
}

// ── Delete confirmation popover ───────────────────────────────────────────────

interface DeletePopoverProps {
  onConfirm: () => void;
  onCancel: () => void;
}

function DeletePopover({ onConfirm, onCancel }: DeletePopoverProps) {
  return (
    <div
      role="dialog"
      aria-label="Confirm delete"
      style={{
        position: "absolute",
        top: "calc(100% + 4px)",
        right: 0,
        background: "var(--color-bc-surface-2)",
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: "8px",
        padding: "0.875rem",
        zIndex: 200,
        boxShadow: "0 4px 16px rgba(0,0,0,0.5)",
        whiteSpace: "nowrap",
      }}
    >
      <p
        style={{
          margin: "0 0 0.625rem",
          fontSize: "0.875rem",
          fontWeight: 600,
          color: "var(--color-bc-text)",
        }}
      >
        Delete this message?
      </p>
      <div style={{ display: "flex", gap: "0.5rem", justifyContent: "flex-end" }}>
        <button
          onClick={onCancel}
          style={{
            border: "1px solid rgba(255,255,255,0.15)",
            background: "transparent",
            color: "var(--color-bc-muted)",
            borderRadius: "4px",
            padding: "0.25rem 0.75rem",
            cursor: "pointer",
            fontSize: "0.8125rem",
          }}
        >
          Cancel
        </button>
        <button
          onClick={onConfirm}
          style={{
            border: "none",
            background: "var(--color-bc-danger)",
            color: "#fff",
            borderRadius: "4px",
            padding: "0.25rem 0.75rem",
            cursor: "pointer",
            fontSize: "0.8125rem",
            fontWeight: 600,
          }}
        >
          Delete
        </button>
      </div>
    </div>
  );
}

// ── Reaction bar ──────────────────────────────────────────────────────────────

interface ReactionBarProps {
  reactions: ReactionInfo[];
  myUserId: string;
  communityId: string;
  channelId: string;
  messageId: string;
  reachable?: boolean;
}

function ReactionBar({
  reactions,
  myUserId,
  communityId,
  channelId,
  messageId,
  reachable = true,
}: ReactionBarProps) {
  if (reactions.length === 0) return null;

  const handleClick = (emoji: string, userIds: string[]) => {
    if (!reachable) return;
    const hasReacted = userIds.includes(myUserId);
    const call = hasReacted
      ? rpcClient.reactionRemove({ community_id: communityId, channel_id: channelId, message_id: messageId, emoji })
      : rpcClient.reactionAdd({ community_id: communityId, channel_id: channelId, message_id: messageId, emoji });
    call.catch(() => {
      toast("Failed to update reaction.", "error");
    });
  };

  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "4px", marginTop: "4px" }}>
      {reactions.map(({ emoji, user_ids }) => {
        const hasReacted = user_ids.includes(myUserId);
        return (
          <button
            key={emoji}
            onClick={() => handleClick(emoji, user_ids)}
            disabled={!reachable}
            title={`${user_ids.length} ${user_ids.length === 1 ? "person" : "people"} reacted`}
            style={{
              border: `1px solid ${hasReacted ? "var(--color-bc-accent)" : "rgba(255,255,255,0.1)"}`,
              background: hasReacted
                ? "rgba(88,101,242,0.2)"
                : "var(--color-bc-surface-2)",
              borderRadius: "12px",
              padding: "2px 8px",
              cursor: reachable ? "pointer" : "not-allowed",
              opacity: reachable ? 1 : 0.5,
              display: "flex",
              alignItems: "center",
              gap: "4px",
              fontSize: "0.8125rem",
              color: "var(--color-bc-text)",
            }}
          >
            <span>{emoji}</span>
            <span style={{ fontWeight: 600 }}>{user_ids.length}</span>
          </button>
        );
      })}
    </div>
  );
}

// ── Action button ─────────────────────────────────────────────────────────────

interface ActionButtonProps {
  icon: ReactNode;
  title: string;
  onClick: () => void;
  danger?: boolean;
}

export function ActionButton({ icon, title, onClick, danger }: ActionButtonProps) {
  const [hov, setHov] = useState(false);
  return (
    <button
      title={title}
      aria-label={title}
      onClick={onClick}
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      style={{
        border: "none",
        background: hov
          ? danger
            ? "rgba(237,66,69,0.15)"
            : "var(--color-bc-surface-3)"
          : "transparent",
        color: hov
          ? danger
            ? "var(--color-bc-danger)"
            : "var(--color-bc-text)"
          : "var(--color-bc-muted)",
        cursor: "pointer",
        padding: "4px",
        borderRadius: "4px",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        transition: "background 0.1s, color 0.1s",
      }}
    >
      {icon}
    </button>
  );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function authorInitials(name: string): string {
  return name
    .split(/\s+/)
    .map((w) => w[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();
}

const AVATAR_COLORS = [
  "#5865f2",
  "#3ba55c",
  "#faa61a",
  "#ed4245",
  "#9c84ef",
  "#00b0f4",
  "#eb459e",
  "#57f287",
];

function avatarColor(userId: string): string {
  let hash = 0;
  for (let i = 0; i < userId.length; i++) {
    hash = ((hash * 31) + userId.charCodeAt(i)) >>> 0;
  }
  return AVATAR_COLORS[hash % AVATAR_COLORS.length];
}

function formatRelativeTime(ts: string): string {
  try {
    const date = new Date(ts);
    const diffMs = Date.now() - date.getTime();
    if (diffMs < 60_000) return "just now";
    if (diffMs < 24 * 3_600_000) return formatDistanceToNow(date, { addSuffix: true });
    return format(date, "MMM d, yyyy");
  } catch {
    return "";
  }
}

function formatAbsoluteTime(ts: string): string {
  try {
    return format(new Date(ts), "PPpp");
  } catch {
    return ts;
  }
}

// ── MessageBubble ─────────────────────────────────────────────────────────────

export interface MessageBubbleProps {
  message: ExtendedMessage;
  /** True when this message is consecutive from the same author within 5 min. */
  isGrouped: boolean;
  myUserId: string;
  communityId: string;
  displayName: string;
  replyToMessage?: ExtendedMessage | null;
  replyToAuthorName?: string;
  isEditing: boolean;
  /** When true, briefly flashes the message background to indicate it was jumped to. */
  isHighlighted?: boolean;
  onReply: (messageId: string) => void;
  /** Called when the user clicks the reply quote to jump to the original message. */
  onScrollToMessage?: (messageId: string) => void;
  onStartEdit: (messageId: string, body: string) => void;
  onSaveEdit: (newBody: string) => void;
  onCancelEdit: () => void;
  /** When false, emoji reactions, edits, and deletes are disabled (seed peer offline). */
  reachable?: boolean;
}

export function MessageBubble({
  message,
  isGrouped,
  myUserId,
  communityId,
  displayName,
  replyToMessage,
  replyToAuthorName,
  isEditing,
  isHighlighted = false,
  onReply,
  onScrollToMessage,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
  reachable = true,
}: MessageBubbleProps) {
  const [hovered, setHovered] = useState(false);
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const emojiPickerRef = useRef<HTMLDivElement>(null);

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
  const [editDraft, setEditDraft] = useState(message.body);
  const editRef = useRef<HTMLTextAreaElement>(null);

  const isOwn = message.author_id === myUserId;

  const handleDeleteConfirm = () => {
    setShowDeleteConfirm(false);
    rpcClient
      .messageDelete({
        community_id: communityId,
        channel_id: message.channel_id,
        message_id: message.id,
      })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : "Failed to delete message.";
        toast(msg, "error");
      });
  };

  const handleReactionAdd = (emoji: string) => {
    rpcClient
      .reactionAdd({
        community_id: communityId,
        channel_id: message.channel_id,
        message_id: message.id,
        emoji,
      })
      .catch(() => {
        toast("Failed to update reaction.", "error");
      });
  };

  const handleCopyText = () => {
    void navigator.clipboard.writeText(message.body);
  };

  const handleSaveEdit = () => {
    const trimmed = editDraft.trim();
    if (trimmed && trimmed !== message.body) {
      onSaveEdit(trimmed);
    } else {
      onCancelEdit();
    }
  };

  const handleEditKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSaveEdit();
    }
    if (e.key === "Escape") {
      onCancelEdit();
    }
  };

  // ── Deleted tombstone ──────────────────────────────────────────────────────
  if (message.deleted) {
    return (
      <div
        style={{
          display: "flex",
          padding: isGrouped ? "1px 1rem 1px 4.5rem" : "0.75rem 1rem 2px",
          gap: "1rem",
          minHeight: isGrouped ? "20px" : "56px",
          alignItems: "center",
        }}
      >
        <div style={{ width: "40px", flexShrink: 0 }} />
        <p
          style={{
            margin: 0,
            color: "var(--color-bc-muted)",
            fontStyle: "italic",
            fontSize: "0.9375rem",
          }}
        >
          Message deleted
        </p>
      </div>
    );
  }

  const paddingTop = isGrouped ? "1px" : "0.75rem";

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => {
        if (!showEmojiPicker && !showDeleteConfirm) setHovered(false);
      }}
      style={{
        display: "flex",
        padding: `${paddingTop} 1rem 2px`,
        gap: "1rem",
        background: isHighlighted
          ? undefined
          : hovered
          ? "rgba(4,4,5,0.07)"
          : "transparent",
        animation: isHighlighted ? "bc-msg-flash 1s ease-out forwards" : undefined,
        position: "relative",
        transition: isHighlighted ? undefined : "background 0.05s",
        opacity: message._status === "pending" ? 0.65 : 1,
      }}
      role="article"
      aria-label={`Message from ${displayName}`}
    >
      {/* Avatar column */}
      <div style={{ width: "40px", flexShrink: 0, paddingTop: isGrouped ? 0 : "2px" }}>
        {!isGrouped ? (
          <div
            aria-hidden="true"
            title={displayName}
            style={{
              width: "40px",
              height: "40px",
              borderRadius: "50%",
              background: avatarColor(message.author_id),
              color: "#fff",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontWeight: 700,
              fontSize: "0.875rem",
              userSelect: "none",
              flexShrink: 0,
            }}
          >
            {authorInitials(displayName)}
          </div>
        ) : null}
      </div>

      {/* Message content */}
      <div style={{ flex: 1, minWidth: 0 }}>
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
            <div
              style={{
                width: "2px",
                minHeight: "16px",
                alignSelf: "stretch",
                background: "var(--color-bc-muted)",
                borderRadius: "1px",
                flexShrink: 0,
              }}
            />
            <span
              style={{
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
                flex: 1,
              }}
            >
              <strong style={{ color: "var(--color-bc-text)", fontWeight: 600 }}>
                {replyToAuthorName ?? replyToMessage.author_id.slice(0, 8)}
              </strong>{" "}
              {replyToMessage.deleted
                ? "Message deleted"
                : replyToMessage.body.slice(0, 100)}
              {!replyToMessage.deleted && replyToMessage.body.length > 100 ? "…" : ""}
            </span>
          </div>
        )}

        {/* Author + timestamp header */}
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
                color: "var(--color-bc-text)",
              }}
            >
              {displayName}
            </span>
            <time
              dateTime={message.timestamp}
              title={formatAbsoluteTime(message.timestamp)}
              style={{
                fontSize: "0.75rem",
                color: "var(--color-bc-muted)",
                userSelect: "none",
              }}
            >
              {formatRelativeTime(message.timestamp)}
            </time>
          </div>
        )}

        {/* Body or inline edit */}
        {isEditing ? (
          <div>
            <textarea
              ref={editRef}
              value={editDraft}
              onChange={(e) => setEditDraft(e.target.value)}
              onKeyDown={handleEditKeyDown}
              autoFocus
              rows={Math.min(Math.max(editDraft.split("\n").length, 2), 8)}
              style={{
                width: "100%",
                background: "var(--color-bc-surface-1)",
                border: "1px solid var(--color-bc-accent)",
                borderRadius: "6px",
                color: "var(--color-bc-text)",
                padding: "0.5rem 0.625rem",
                fontSize: "0.9375rem",
                resize: "vertical",
                outline: "none",
                fontFamily: "inherit",
                lineHeight: 1.375,
              }}
            />
            <p
              style={{
                margin: "4px 0 0",
                fontSize: "0.75rem",
                color: "var(--color-bc-muted)",
              }}
            >
              Enter to save · Shift+Enter for newline · Escape to cancel
            </p>
          </div>
        ) : (
          <div>
            <div className="message-body">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {message.body}
              </ReactMarkdown>
            </div>
            {message.edited_at && (
              <span
                style={{
                  fontSize: "0.6875rem",
                  color: "var(--color-bc-muted)",
                  marginLeft: "4px",
                }}
              >
                (edited)
              </span>
            )}
            {message._status === "failed" && message._onRetry && (
              <button
                onClick={message._onRetry}
                style={{
                  border: "none",
                  background: "transparent",
                  color: "var(--color-bc-danger)",
                  cursor: "pointer",
                  fontSize: "0.75rem",
                  padding: "0 4px",
                  textDecoration: "underline",
                }}
              >
                Failed — Retry
              </button>
            )}
          </div>
        )}

        {/* Reactions */}
        <ReactionBar
          reactions={message.reactions}
          myUserId={myUserId}
          communityId={communityId}
          channelId={message.channel_id}
          messageId={message.id}
          reachable={reachable}
        />
      </div>

      {/* Hover action bar */}
      {(hovered || showDeleteConfirm) && !isEditing && message._status !== "pending" && (
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
          {reachable && (
          <div ref={emojiPickerRef} style={{ position: "relative" }}>
            <ActionButton
              icon={<Smile size={16} />}
              title="Add reaction"
              onClick={() => {
                setShowEmojiPicker((v) => !v);
                setShowDeleteConfirm(false);
              }}
            />
            {showEmojiPicker && (
              <EmojiPicker
                onSelect={handleReactionAdd}
                onClose={() => setShowEmojiPicker(false)}
              />
            )}
          </div>
          )}

          {/* Reply */}
          <ActionButton
            icon={<Reply size={16} />}
            title="Reply"
            onClick={() => {
              onReply(message.id);
              setHovered(false);
            }}
          />

          {/* Edit (own messages only) */}
          {isOwn && reachable && (
            <ActionButton
              icon={<Edit2 size={16} />}
              title="Edit message"
              onClick={() => {
                setEditDraft(message.body);
                onStartEdit(message.id, message.body);
                setHovered(false);
              }}
            />
          )}

          {/* Copy */}
          <ActionButton
            icon={<Copy size={16} />}
            title="Copy text"
            onClick={handleCopyText}
          />

          {/* Delete (own messages only) */}
          {isOwn && reachable && (
            <div style={{ position: "relative" }}>
              <ActionButton
                icon={<Trash2 size={16} />}
                title="Delete message"
                danger
                onClick={() => {
                  setShowDeleteConfirm((v) => !v);
                  setShowEmojiPicker(false);
                }}
              />
              {showDeleteConfirm && (
                <DeletePopover
                  onConfirm={handleDeleteConfirm}
                  onCancel={() => setShowDeleteConfirm(false)}
                />
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
