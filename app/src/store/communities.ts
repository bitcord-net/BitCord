import { create } from "zustand";
import type { CommunityInfo, ChannelInfo, MemberInfo } from "../lib/rpc-types";
import { rpcClient } from "../hooks/useRpc";

interface CommunitiesState {
  communities: CommunityInfo[];
  activeCommunityId: string | null;
  channels: Record<string, ChannelInfo[]>; // communityId -> channels
  members: Record<string, MemberInfo[]>;   // communityId -> members
  syncProgress: Record<string, number>;    // channelId -> progress (0..1)
  isLoaded: boolean;
  load: () => Promise<void>;
  setActive: (id: string) => void;
  loadChannels: (communityId: string) => Promise<void>;
  loadMembers: (communityId: string) => Promise<void>;
  setSyncProgress: (channelId: string, progress: number) => void;
  addCommunity: (c: CommunityInfo) => void;
  removeCommunity: (id: string) => void;
  updateCommunity: (c: CommunityInfo) => void;
  addChannel: (communityId: string, channel: ChannelInfo) => void;
  removeChannel: (communityId: string, channelId: string) => void;
  reorderChannels: (communityId: string, orderedIds: string[]) => void;
}

export const useCommunitiesStore = create<CommunitiesState>((set) => ({
  communities: [],
  activeCommunityId: null,
  channels: {},
  members: {},
  syncProgress: {},
  isLoaded: false,

  load: async () => {
    try {
      const communities = await rpcClient.communityList();
      set({ communities, isLoaded: true });
    } catch {
      set({ communities: [], isLoaded: true });
    }
  },

  setActive: (id) => set({ activeCommunityId: id }),

  loadChannels: async (communityId) => {
    try {
      const channels = await rpcClient.channelList(communityId);
      set((s) => ({ channels: { ...s.channels, [communityId]: channels } }));
    } catch {
      // keep existing
    }
  },

  loadMembers: async (communityId) => {
    try {
      const members = await rpcClient.memberList(communityId);
      set((s) => ({ members: { ...s.members, [communityId]: members } }));
    } catch {
      // keep existing
    }
  },

  setSyncProgress: (channelId, progress) =>
    set((s) => ({
      syncProgress: { ...s.syncProgress, [channelId]: progress },
    })),

  addCommunity: (c) =>
    set((s) => ({ communities: [...s.communities, c] })),

  removeCommunity: (id) =>
    set((s) => {
      const { [id]: _removedChannels, ...remainingChannels } = s.channels;
      const { [id]: _removedMembers, ...remainingMembers } = s.members;
      return {
        communities: s.communities.filter((c) => c.id !== id),
        activeCommunityId: s.activeCommunityId === id ? null : s.activeCommunityId,
        channels: remainingChannels,
        members: remainingMembers,
      };
    }),

  updateCommunity: (c) =>
    set((s) => ({
      communities: s.communities.map((existing) =>
        existing.id === c.id ? c : existing
      ),
    })),

  addChannel: (communityId, channel) =>
    set((s) => ({
      channels: {
        ...s.channels,
        [communityId]: [...(s.channels[communityId] ?? []), channel],
      },
    })),

  removeChannel: (communityId, channelId) =>
    set((s) => ({
      channels: {
        ...s.channels,
        [communityId]: (s.channels[communityId] ?? []).filter(
          (ch) => ch.id !== channelId
        ),
      },
      communities: s.communities.map((c) =>
        c.id === communityId
          ? { ...c, channel_ids: c.channel_ids.filter((id) => id !== channelId) }
          : c
      ),
    })),

  reorderChannels: (communityId, orderedIds) =>
    set((s) => {
      const existing = s.channels[communityId] ?? [];
      const reordered = orderedIds
        .map((id) => existing.find((ch) => ch.id === id))
        .filter((ch): ch is ChannelInfo => ch !== undefined);
      return { channels: { ...s.channels, [communityId]: reordered } };
    }),
}));
