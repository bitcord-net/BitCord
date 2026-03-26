import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * Subscribe to a Tauri backend event for the lifetime of the component.
 *
 * The handler ref is updated on every render so callers never need to
 * memoize the callback.  Falls back to a no-op when running outside of
 * a Tauri webview (e.g. during Vite dev-server previews).
 *
 * @param event  Tauri event name (e.g. `"message:new"`)
 * @param handler  Called with the event payload whenever the event fires
 */
export function useTauriEvent<T = unknown>(
  event: string,
  handler: (payload: T) => void
): void {
  const handlerRef = useRef(handler);
  useEffect(() => {
    handlerRef.current = handler;
  });

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<T>(event, (ev) => {
      handlerRef.current(ev.payload);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        // Not running inside Tauri — silently ignore.
      });

    return () => {
      unlisten?.();
    };
  }, [event]);
}
