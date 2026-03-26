import { useEffect, useState } from "react";
import { BitCordClient } from "../lib/rpc-client";

// Singleton — one connection for the lifetime of the app.
export const rpcClient = new BitCordClient(
  import.meta.env.VITE_RPC_URL ?? "ws://127.0.0.1:7331"
);

/**
 * Returns the singleton RPC client and a reactive `isConnected` flag.
 * The first call initiates the WebSocket connection; subsequent calls
 * share the same client instance.
 */
export function useRpc(): { client: BitCordClient; isConnected: boolean } {
  const [isConnected, setIsConnected] = useState(rpcClient.isConnected);

  useEffect(() => {
    rpcClient.connect();

    const id = setInterval(() => {
      setIsConnected(rpcClient.isConnected);
    }, 300);

    return () => clearInterval(id);
  }, []);

  return { client: rpcClient, isConnected };
}
