import { create } from "zustand";

export interface PresenceEntry {
  status: string;
  last_seen: string;
}

interface PresenceState {
  /** userId → presence entry */
  presence: Record<string, PresenceEntry>;
  update: (userId: string, status: string, lastSeen: string) => void;
  getStatus: (userId: string) => string;
  /** Mark entries whose last_seen is older than `olderThanMs` as offline. */
  expireStale: (olderThanMs: number) => void;
}

export const usePresenceStore = create<PresenceState>((set, get) => ({
  presence: {},

  update: (userId, status, lastSeen) =>
    set((s) => ({
      presence: {
        ...s.presence,
        [userId]: { status, last_seen: lastSeen },
      },
    })),

  getStatus: (userId) => get().presence[userId]?.status ?? "offline",

  expireStale: (olderThanMs) => {
    const cutoff = Date.now() - olderThanMs;
    set((s) => {
      let changed = false;
      const updated: Record<string, PresenceEntry> = {};
      for (const [uid, entry] of Object.entries(s.presence)) {
        if (entry.status !== "offline" && new Date(entry.last_seen).getTime() < cutoff) {
          updated[uid] = { ...entry, status: "offline" };
          changed = true;
        } else {
          updated[uid] = entry;
        }
      }
      return changed ? { presence: updated } : s;
    });
  },
}));
