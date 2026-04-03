import { create } from "zustand";
import type { IdentityInfo } from "../lib/rpc-types";
import { rpcClient } from "../hooks/useRpc";

interface IdentityState {
  identity: IdentityInfo | null;
  serverEnabled: boolean;
  isLoaded: boolean;
  load: () => Promise<void>;
  setDisplayName: (name: string) => Promise<void>;
  setStatus: (status: IdentityInfo["status"]) => Promise<void>;
}

export const useIdentityStore = create<IdentityState>((set) => ({
  identity: null,
  serverEnabled: true,
  isLoaded: false,

  load: async () => {
    try {
      const [identity, config] = await Promise.all([
        rpcClient.identityGet(),
        rpcClient.nodeGetConfig(),
      ]);
      set({ identity, serverEnabled: config.node_mode !== "gossip_client", isLoaded: true });
    } catch {
      set({ identity: null, serverEnabled: false, isLoaded: true });
    }
  },

  setDisplayName: async (name) => {
    await rpcClient.identitySetDisplayName({ display_name: name });
    set((s) => ({
      identity: s.identity ? { ...s.identity, display_name: name } : null,
    }));
  },

  setStatus: async (status) => {
    await rpcClient.identitySetStatus({ status });
    set((s) => ({
      identity: s.identity ? { ...s.identity, status } : null,
    }));
  },
}));
