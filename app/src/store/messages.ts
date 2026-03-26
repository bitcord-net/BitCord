import { create } from "zustand";
import type { MessageInfo } from "../lib/rpc-types";

interface MessagesState {
  /** channelId → ordered message list */
  messages: Record<string, MessageInfo[]>;
  /** channelId → unread count */
  unreadCounts: Record<string, number>;
  setHistory: (channelId: string, msgs: MessageInfo[]) => void;
  append: (msg: MessageInfo) => void;
  update: (channelId: string, messageId: string, changes: Partial<MessageInfo>) => void;
  tombstone: (channelId: string, messageId: string) => void;
  incrementUnread: (channelId: string) => void;
  clearUnread: (channelId: string) => void;
}

export const useMessagesStore = create<MessagesState>((set) => ({
  messages: {},
  unreadCounts: {},

  setHistory: (channelId, msgs) =>
    set((s) => ({ messages: { ...s.messages, [channelId]: msgs } })),

  append: (msg) =>
    set((s) => {
      const existing = s.messages[msg.channel_id] ?? [];
      // Avoid duplicates (optimistic insert may already be present).
      if (existing.some((m) => m.id === msg.id)) return s;
      return {
        messages: { ...s.messages, [msg.channel_id]: [...existing, msg] },
      };
    }),

  update: (channelId, messageId, changes) =>
    set((s) => ({
      messages: {
        ...s.messages,
        [channelId]: (s.messages[channelId] ?? []).map((m) =>
          m.id === messageId ? { ...m, ...changes } : m
        ),
      },
    })),

  tombstone: (channelId, messageId) =>
    set((s) => ({
      messages: {
        ...s.messages,
        [channelId]: (s.messages[channelId] ?? []).map((m) =>
          m.id === messageId ? { ...m, deleted: true } : m
        ),
      },
    })),

  incrementUnread: (channelId) =>
    set((s) => ({
      unreadCounts: {
        ...s.unreadCounts,
        [channelId]: (s.unreadCounts[channelId] ?? 0) + 1,
      },
    })),

  clearUnread: (channelId) =>
    set((s) => ({
      unreadCounts: { ...s.unreadCounts, [channelId]: 0 },
    })),
}));
