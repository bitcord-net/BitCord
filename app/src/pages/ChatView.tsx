import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { useParams } from "react-router-dom";
import { useIsMobile } from "../hooks/useIsMobile";
import { Loader2, Volume2, MicOff, Megaphone } from "lucide-react";
import { useCommunitiesStore } from "../store/communities";
import { useMessagesStore } from "../store/messages";
import { useIdentityStore } from "../store/identity";
import { useSubscription } from "../hooks/useSubscription";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { rpcClient, useRpc } from "../hooks/useRpc";
import { toast } from "../store/toast";
import { ChannelHeader } from "../components/ChannelHeader";
import { MessageList } from "../components/MessageList";
import { MessageInput } from "../components/MessageInput";
import type { ExtendedMessage } from "../components/MessageBubble";
import type { MessageInfo, ReactionUpdatedData } from "../lib/rpc-types";

const HISTORY_LIMIT = 50;

type PendingMessage = ExtendedMessage & { _status: "pending" | "failed"; _onRetry: () => void };

export function ChatView() {
  const { cid, chid } = useParams<{ cid: string; chid: string }>();

  const { isConnected } = useRpc();
  const { communities, channels, members, syncProgress, loadMembers } = useCommunitiesStore();
  const { messages, setHistory, append, update, clearUnread } = useMessagesStore();
  const { identity } = useIdentityStore();

  const community = communities.find((c) => c.id === cid);

  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [replyToId, setReplyToId] = useState<string | null>(null);
  const [pendingMessages, setPendingMessages] = useState<PendingMessage[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [isInitialLoading, setIsInitialLoading] = useState(true);

  // Track last channel loaded to avoid duplicate fetches
  const loadedChannelRef = useRef<string | null>(null);

  // Ensure members are loaded for announcement channel permission checks.
  useEffect(() => {
    if (!cid || !isConnected) return;
    if (!members[cid]) {
      void loadMembers(cid);
    }
  }, [cid, isConnected, members, loadMembers]);

  const channelList = useMemo(
    () => (cid ? (channels[cid] ?? []) : []),
    [cid, channels]
  );
  const memberList = useMemo(
    () => (cid ? (members[cid] ?? []) : []),
    [cid, members]
  );
  const channel = channelList.find((c) => c.id === chid);
  const channelMessages = useMemo(
    () => (chid ? (messages[chid] ?? []) : []),
    [chid, messages]
  );

  // Combined list: store messages + pending
  const allMessages: ExtendedMessage[] = useMemo(() => {
    // Deduplicate: pending messages that already appear in store can be removed
    const storeIds = new Set(channelMessages.map((m) => m.id));
    const filteredPending = pendingMessages.filter((p) => !storeIds.has(p.id));
    return [...channelMessages, ...filteredPending];
  }, [channelMessages, pendingMessages]);

  // Member name lookup
  const getMemberName = useCallback(
    (userId: string): string => {
      const found = memberList.find((m) => m.user_id === userId);
      if (found) return found.display_name;
      if (userId === identity?.peer_id) return identity.display_name ?? userId.slice(0, 8);
      return userId.slice(0, 8);
    },
    [memberList, identity]
  );

  // Load initial history when channel changes (gate on isConnected to survive refresh)
  useEffect(() => {
    if (!cid || !chid || !isConnected) return;
    if (loadedChannelRef.current === chid) return;
    loadedChannelRef.current = chid;

    setIsInitialLoading(true);
    setPendingMessages([]);
    setEditingMessageId(null);
    setReplyToId(null);

    void rpcClient
      .messageGetHistory({ community_id: cid, channel_id: chid, before: null, limit: HISTORY_LIMIT })
      .then((msgs) => {
        setHistory(chid, msgs);
        setHasMore(msgs.length === HISTORY_LIMIT);
        clearUnread(chid);
      })
      .catch(() => {
        setHistory(chid, []);
        setHasMore(false);
      })
      .finally(() => setIsInitialLoading(false));
  }, [cid, chid, isConnected, setHistory, clearUnread]);

  // Subscribe to new messages via JSON-RPC (existing path through AppState broadcaster)
  useSubscription("message_new", (ev) => {
    if (!cid || !chid) return;
    if (ev.data.channel_id !== chid) return;

    void rpcClient
      .messageGetHistory({ community_id: cid, channel_id: chid, before: null, limit: 5 })
      .then((msgs) => msgs.forEach(append))
      .catch(() => {});
  });

  // Also listen for Tauri native events from the embedded QUIC node.
  // This covers messages that arrive via NodeClient push and were emitted
  // by the push relay task in lib.rs.
  useTauriEvent<{ channel_id: string; seq: number; author_id: string }>(
    "message:new",
    (payload) => {
      if (!cid || !chid) return;
      if (payload.channel_id !== chid) return;

      void rpcClient
        .messageGetHistory({ community_id: cid, channel_id: chid, before: null, limit: 5 })
        .then((msgs) => msgs.forEach(append))
        .catch(() => {});
    }
  );

  // Reload full history when P2P channel history sync completes for this channel
  useSubscription("channel_history_synced", (ev) => {
    if (!cid || !chid) return;
    if (ev.data.channel_id !== chid) return;
    void rpcClient
      .messageGetHistory({ community_id: cid, channel_id: chid, before: null, limit: HISTORY_LIMIT })
      .then((msgs) => {
        setHistory(chid, msgs);
        setHasMore(msgs.length === HISTORY_LIMIT);
      })
      .catch(() => {});
  });

  // Update reactions on a specific message when any peer reacts or un-reacts
  useSubscription("reaction_updated", (ev: { data: ReactionUpdatedData }) => {
    if (!chid || ev.data.channel_id !== chid) return;
    update(ev.data.channel_id, ev.data.message_id, { reactions: ev.data.reactions });
  });

  // Mark channel read when it becomes active
  useEffect(() => {
    if (!cid || !chid) return;
    clearUnread(chid);
    const lastMsg = channelMessages[channelMessages.length - 1];
    if (lastMsg) {
      void rpcClient.markRead({ community_id: cid, channel_id: chid, message_id: lastMsg.id }).catch(() => {});
    }
  }, [cid, chid, clearUnread, channelMessages.length]); // eslint-disable-line react-hooks/exhaustive-deps

  // Load older messages
  const handleLoadMore = useCallback(async () => {
    if (!cid || !chid || !hasMore || isLoadingMore) return;
    const oldest = channelMessages[0];
    if (!oldest) return;

    setIsLoadingMore(true);
    try {
      const older = await rpcClient.messageGetHistory({
        community_id: cid,
        channel_id: chid,
        before: oldest.id,
        limit: HISTORY_LIMIT,
      });
      if (older.length > 0) {
        setHistory(chid, [...older, ...channelMessages]);
        setHasMore(older.length === HISTORY_LIMIT);
      } else {
        setHasMore(false);
      }
    } catch {
      // keep current state
    } finally {
      setIsLoadingMore(false);
    }
  }, [cid, chid, hasMore, isLoadingMore, channelMessages, setHistory]);

  // Send a message
  const handleSend = useCallback(
    async (body: string, replyToMessageId?: string) => {
      if (!cid || !chid) return;

      const tempId = `pending-${Date.now()}-${Math.random()}`;
      const now = new Date().toISOString();
      const myId = identity?.peer_id ?? "me";

      const doSend = async (id: string) => {
        // Remove old pending entry if retrying
        setPendingMessages((prev) => prev.filter((p) => p.id !== id));

        // Add new pending entry
        const pending: PendingMessage = {
          id: tempId,
          channel_id: chid,
          community_id: cid,
          author_id: myId,
          timestamp: now,
          body,
          reply_to: replyToMessageId ?? null,
          edited_at: null,
          deleted: false,
          reactions: [],
          _status: "pending",
          _onRetry: () => void doSend(tempId),
        };
        setPendingMessages((prev) => [...prev.filter((p) => p.id !== tempId), pending]);

        try {
          const result = await rpcClient.messageSend({
            community_id: cid,
            channel_id: chid,
            body,
            reply_to: replyToMessageId ?? null,
          });
          append(result);
          setPendingMessages((prev) => prev.filter((p) => p.id !== tempId));
        } catch {
          setPendingMessages((prev) =>
            prev.map((p) =>
              p.id === tempId ? { ...p, _status: "failed" } : p
            )
          );
        }
      };

      setReplyToId(null);
      await doSend(tempId);
    },
    [cid, chid, identity, append]
  );

  // Edit a message
  const handleSaveEdit = useCallback(
    async (messageId: string, newBody: string) => {
      if (!cid || !chid) return;
      setEditingMessageId(null);
      try {
        await rpcClient.messageEdit({
          community_id: cid,
          channel_id: chid,
          message_id: messageId,
          body: newBody,
        });
        const { update } = useMessagesStore.getState();
        update(chid, messageId, { body: newBody, edited_at: new Date().toISOString() });
      } catch {
        toast("Failed to edit message.", "error");
      }
    },
    [cid, chid]
  );

  // Resolve replyTo message
  const replyToMessage = replyToId
    ? (channelMessages.find((m) => m.id === replyToId) as MessageInfo | undefined) ?? null
    : null;

  // Announcement channels are read-only for regular members.
  // While members are still loading, allow input so admins aren't briefly blocked.
  const myMember = memberList.find((m) => m.user_id === identity?.peer_id);
  const isAnnouncementReadOnly =
    channel?.kind === "announcement" &&
    memberList.length > 0 &&
    !myMember?.roles.some((r) => r === "admin" || r === "moderator");

  const isMobile = useIsMobile();
  const progress = chid ? syncProgress[chid] : undefined;
  const isSyncing = progress !== undefined && progress < 1.0;

  // ── Loading state ────────────────────────────────────────────────────────────
  if (!channel) {
    return (
      <div
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: "var(--color-bc-muted)",
          flexDirection: "column",
          gap: "0.5rem",
        }}
      >
        <Loader2 size={24} style={{ animation: "spin 1s linear infinite" }} />
        <span>Loading channel…</span>
        <style>{`@keyframes spin { from{transform:rotate(0deg)} to{transform:rotate(360deg)} }`}</style>
      </div>
    );
  }

  // Voice channels are not yet implemented — show a Coming Soon screen.
  if (channel.kind === "voice") {
    return (
      <div style={{ display: "flex", flexDirection: "column", height: "100%", overflow: "hidden" }}>
        {!isMobile && <ChannelHeader channel={channel} memberCount={memberList.length} />}
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: "1rem",
            color: "var(--color-bc-muted)",
            background: "var(--color-bc-surface-3)",
          }}
        >
          <div
            style={{
              width: "64px",
              height: "64px",
              borderRadius: "50%",
              background: "var(--color-bc-surface-2)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              position: "relative",
            }}
          >
            <Volume2 size={28} aria-hidden="true" />
            <MicOff
              size={16}
              aria-hidden="true"
              style={{
                position: "absolute",
                bottom: "4px",
                right: "4px",
                color: "var(--color-bc-danger)",
              }}
            />
          </div>
          <div style={{ textAlign: "center" }}>
            <p style={{ margin: "0 0 0.25rem", fontWeight: 700, fontSize: "1.125rem", color: "var(--color-bc-text)" }}>
              Voice channels coming soon
            </p>
            <p style={{ margin: 0, fontSize: "0.875rem" }}>
              Real-time voice is not yet implemented.
            </p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
      }}
    >
      {!isMobile && <ChannelHeader channel={channel} memberCount={memberList.length} />}

      {isSyncing && (
        <div
          style={{
            padding: "4px 12px",
            background: "var(--color-bc-accent)",
            color: "#fff",
            fontSize: "0.75rem",
            fontWeight: 600,
            display: "flex",
            alignItems: "center",
            gap: "8px",
            zIndex: 10,
          }}
        >
          <Loader2 size={12} style={{ animation: "spin 1s linear infinite" }} />
          <span>Syncing history… {Math.round(progress * 100)}%</span>
        </div>
      )}

      {isInitialLoading ? (
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "var(--color-bc-muted)",
            flexDirection: "column",
            gap: "0.5rem",
          }}
        >
          <Loader2 size={24} style={{ animation: "spin 1s linear infinite" }} />
          <span>Loading messages…</span>
          <style>{`@keyframes spin { from{transform:rotate(0deg)} to{transform:rotate(360deg)} }`}</style>
        </div>
      ) : (
        <MessageList
          messages={allMessages}
          myUserId={identity?.peer_id ?? ""}
          communityId={cid ?? ""}
          getMemberName={getMemberName}
          editingMessageId={editingMessageId}
          onStartEdit={(id, body) => {
            setEditingMessageId(id);
            setReplyToId(null);
            // Seed the edit draft — MessageBubble manages its own draft state
            void body;
          }}
          onSaveEdit={handleSaveEdit}
          onCancelEdit={() => setEditingMessageId(null)}
          onReply={(id) => {
            setReplyToId(id);
            setEditingMessageId(null);
          }}
          onLoadMore={handleLoadMore}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          reachable={community?.reachable ?? true}
        />
      )}

      <div style={{ height: "15px", flexShrink: 0 }} />

      {isAnnouncementReadOnly ? (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "0.5rem",
            padding: "0.75rem 1rem",
            background: "var(--color-bc-surface-2)",
            borderTop: "1px solid var(--color-bc-border)",
            color: "var(--color-bc-muted)",
            fontSize: "0.875rem",
          }}
        >
          <Megaphone size={16} aria-hidden="true" />
          <span>This is an announcement channel. Only admins and moderators can post here.</span>
        </div>
      ) : (
        <MessageInput
          channelName={channel.name}
          replyToMessage={replyToMessage}
          replyToAuthorName={replyToMessage ? getMemberName(replyToMessage.author_id) : undefined}
          onClearReply={() => setReplyToId(null)}
          onSend={handleSend}
          members={memberList}
          channels={channelList}
          disabled={isInitialLoading || !(community?.reachable ?? true)}
        />
      )}
    </div>
  );
}
