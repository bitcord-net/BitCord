import { useEffect, useState, useCallback, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useRpc, rpcClient } from "../hooks/useRpc";
import { useIdentityStore } from "../store/identity";
import { useTauriEvent } from "../hooks/useTauriEvent";

const CONNECT_TIMEOUT_MS = 15_000;
const BACKEND_POLL_MS = 2_000;

interface Props {
  children: ReactNode;
}

/**
 * Loads identity on first connection, then redirects to /onboarding
 * if the identity has no display_name set (first-run state).
 *
 * Shows a timeout screen with a Retry button if the backend is
 * unreachable after CONNECT_TIMEOUT_MS.
 */
export function ProtectedRoute({ children }: Props) {
  const navigate = useNavigate();
  const { isConnected } = useRpc();
  const { identity, isLoaded, load } = useIdentityStore();
  const [timedOut, setTimedOut] = useState(false);

  // Listen for backend_status events from Tauri.
  useTauriEvent<{ status: string }>("backend_status", (payload) => {
    if (payload.status === "first_run") {
      navigate("/onboarding", { replace: true });
    } else if (payload.status === "needs_unlock") {
      navigate("/unlock", { replace: true });
    }
    // "ready" — backend started, RPC will connect and trigger identity load
  });

  // Poll get_backend_status while loading.
  //
  // On Android the background startup task can be slow or silently skipped,
  // so a single on-mount query isn't reliable. Polling every BACKEND_POLL_MS
  // ensures we catch the status transition even if the initial event was lost.
  useEffect(() => {
    if (isLoaded) return;

    const check = () => {
      invoke<{ status: string }>("get_backend_status")
        .then((payload) => {
          if (payload.status === "first_run") {
            navigate("/onboarding", { replace: true });
          } else if (payload.status === "needs_unlock") {
            navigate("/unlock", { replace: true });
          }
          // "auto_unlocking" or "ready" — keep waiting for RPC connection
        })
        .catch(() => {
          // Backend not yet ready; keep polling
        });
    };

    check(); // immediate check on mount / when isLoaded resets
    const id = setInterval(check, BACKEND_POLL_MS);
    return () => clearInterval(id);
  }, [isLoaded, navigate]);

  // Start timeout clock as soon as we're waiting to load.
  useEffect(() => {
    if (isLoaded) return;
    const id = setTimeout(() => setTimedOut(true), CONNECT_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [isLoaded]);

  useEffect(() => {
    if (isConnected && !isLoaded) {
      void load();
    }
  }, [isConnected, isLoaded, load]);

  useEffect(() => {
    if (isLoaded && (!identity || !identity.display_name)) {
      navigate("/onboarding", { replace: true });
    }
  }, [isLoaded, identity, navigate]);

  const handleRetry = useCallback(() => {
    setTimedOut(false);
    rpcClient.forceReconnect();
  }, []);

  if (!isLoaded) {
    if (timedOut) {
      return (
        <div
          style={{
            height: "100%",
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: "1rem",
            background: "var(--color-bc-base)",
            color: "var(--color-bc-muted)",
          }}
        >
          <p
            style={{
              margin: 0,
              fontWeight: 700,
              fontSize: "1rem",
              color: "var(--color-bc-text)",
            }}
          >
            BitCord failed to start
          </p>
          <p style={{ margin: 0, fontSize: "0.875rem", textAlign: "center", maxWidth: "320px" }}>
            The local node could not be reached. This can happen if the app
            didn't have enough time to initialise — tap Retry to try again.
          </p>
          <button
            onClick={handleRetry}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.5rem",
              padding: "0.5rem 1.25rem",
              background: "var(--color-bc-accent)",
              border: "none",
              borderRadius: "6px",
              color: "#fff",
              fontWeight: 600,
              fontSize: "0.9375rem",
              cursor: "pointer",
            }}
          >
            <RefreshCw size={15} aria-hidden="true" />
            Retry
          </button>
        </div>
      );
    }

    return (
      <div
        style={{
          height: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--color-bc-base)",
          color: "var(--color-bc-muted)",
        }}
        aria-live="polite"
        aria-label="Connecting to BitCord node"
      >
        <span>Starting BitCord…</span>
      </div>
    );
  }

  if (!identity || !identity.display_name) return null;

  return <>{children}</>;
}
