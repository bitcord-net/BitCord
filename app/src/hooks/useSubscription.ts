import { useEffect, useRef } from "react";
import type { PushEventPayload } from "../lib/rpc-types";
import { rpcClient } from "./useRpc";

/**
 * Subscribes to a server-push event type for the lifetime of the component.
 * The `handler` ref is updated on every render so callers never need to
 * memoize the callback.
 */
export function useSubscription<T extends PushEventPayload["type"]>(
  eventType: T,
  handler: (event: Extract<PushEventPayload, { type: T }>) => void
): void {
  const handlerRef = useRef(handler);
  // Keep the ref up-to-date without triggering the subscription effect.
  useEffect(() => {
    handlerRef.current = handler;
  });

  useEffect(() => {
    return rpcClient.subscribe(eventType, (event) => {
      handlerRef.current(event);
    });
  }, [eventType]);
}
