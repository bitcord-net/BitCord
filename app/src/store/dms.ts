import { create } from "zustand";
import type { DmMessageInfo } from "../lib/rpc-types";

export interface DmConversation {
  peerId: string;
  displayName: string;
  lastMessage: DmMessageInfo | null;
  unread: number;
}

export type DmReactionGroup = { emoji: string; userIds: string[] };

interface DmsState {
  conversations: DmConversation[];
  messages: Record<string, DmMessageInfo[]>; // peerId → messages (asc by timestamp)
  localReactions: Record<string, DmReactionGroup[]>; // msgId → reactions (local-only)

  // Conversation management
  upsertConversation: (peerId: string, displayName: string, lastMsg?: DmMessageInfo) => void;
  removeConversation: (peerId: string) => void;

  // Messages
  setHistory: (peerId: string, msgs: DmMessageInfo[]) => void;
  appendMessage: (peerId: string, msg: DmMessageInfo) => void;
  updateMessage: (peerId: string, msgId: string, changes: Partial<DmMessageInfo>) => void;

  // Unread
  incrementUnread: (peerId: string) => void;
  clearUnread: (peerId: string) => void;

  // Local reactions (not persisted to backend)
  toggleReaction: (msgId: string, emoji: string, userId: string) => void;
}

const CONVERSATIONS_KEY = "bc_dm_conversations";

function loadConversations(): DmConversation[] {
  try {
    const raw = localStorage.getItem(CONVERSATIONS_KEY);
    if (!raw) return [];
    return JSON.parse(raw) as DmConversation[];
  } catch {
    return [];
  }
}

function saveConversations(conversations: DmConversation[]): void {
  try {
    localStorage.setItem(CONVERSATIONS_KEY, JSON.stringify(conversations));
  } catch {
    // ignore quota errors
  }
}

export const useDmsStore = create<DmsState>((set, get) => ({
  conversations: loadConversations(),
  messages: {},
  localReactions: {},

  upsertConversation: (peerId, displayName, lastMsg) =>
    set((s) => {
      const existing = s.conversations.find((c) => c.peerId === peerId);
      let conversations: DmConversation[];
      if (existing) {
        conversations = s.conversations.map((c) =>
          c.peerId === peerId
            ? { ...c, displayName, lastMessage: lastMsg ?? c.lastMessage }
            : c
        );
      } else {
        conversations = [
          ...s.conversations,
          { peerId, displayName, lastMessage: lastMsg ?? null, unread: 0 },
        ];
      }
      saveConversations(conversations);
      return { conversations };
    }),

  removeConversation: (peerId) =>
    set((s) => {
      const conversations = s.conversations.filter((c) => c.peerId !== peerId);
      saveConversations(conversations);
      return { conversations };
    }),

  setHistory: (peerId, msgs) =>
    set((s) => ({
      messages: { ...s.messages, [peerId]: msgs },
    })),

  appendMessage: (peerId, msg) =>
    set((s) => {
      const existing = s.messages[peerId] ?? [];
      // Avoid duplicates
      if (existing.some((m) => m.id === msg.id)) return s;
      const updated = [...existing, msg].sort(
        (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime()
      );
      // Upsert conversation: create if new peer, update lastMessage otherwise
      const convExists = s.conversations.some((c) => c.peerId === peerId);
      let conversations: DmConversation[];
      if (convExists) {
        conversations = s.conversations.map((c) =>
          c.peerId === peerId ? { ...c, lastMessage: msg } : c
        );
      } else {
        conversations = [
          ...s.conversations,
          { peerId, displayName: peerId.slice(0, 12) + "…", lastMessage: msg, unread: 0 },
        ];
      }
      saveConversations(conversations);
      return { messages: { ...s.messages, [peerId]: updated }, conversations };
    }),

  updateMessage: (peerId, msgId, changes) =>
    set((s) => {
      const existing = s.messages[peerId];
      if (!existing) return s;
      return {
        messages: {
          ...s.messages,
          [peerId]: existing.map((m) => (m.id === msgId ? { ...m, ...changes } : m)),
        },
      };
    }),

  incrementUnread: (peerId) =>
    set((s) => {
      const conversations = s.conversations.map((c) =>
        c.peerId === peerId ? { ...c, unread: c.unread + 1 } : c
      );
      saveConversations(conversations);
      return { conversations };
    }),

  clearUnread: (peerId) => {
    const { conversations } = get();
    if (!conversations.find((c) => c.peerId === peerId)) return;
    set((s) => {
      const conversations = s.conversations.map((c) =>
        c.peerId === peerId ? { ...c, unread: 0 } : c
      );
      saveConversations(conversations);
      return { conversations };
    });
  },

  toggleReaction: (msgId, emoji, userId) =>
    set((s) => {
      const existing = s.localReactions[msgId] ?? [];
      const group = existing.find((r) => r.emoji === emoji);
      let updated: DmReactionGroup[];
      if (group) {
        const hasReacted = group.userIds.includes(userId);
        if (hasReacted) {
          const newUserIds = group.userIds.filter((id) => id !== userId);
          updated = newUserIds.length > 0
            ? existing.map((r) => r.emoji === emoji ? { ...r, userIds: newUserIds } : r)
            : existing.filter((r) => r.emoji !== emoji);
        } else {
          updated = existing.map((r) =>
            r.emoji === emoji ? { ...r, userIds: [...r.userIds, userId] } : r
          );
        }
      } else {
        updated = [...existing, { emoji, userIds: [userId] }];
      }
      return { localReactions: { ...s.localReactions, [msgId]: updated } };
    }),
}));
