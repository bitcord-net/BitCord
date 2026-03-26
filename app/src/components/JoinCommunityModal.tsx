import React, { useState, useRef, useEffect } from "react";
import { X, Loader2 } from "lucide-react";
import { rpcClient } from "../hooks/useRpc";
import type { CommunityInfo } from "../lib/rpc-types";

interface Props {
  onClose: () => void;
  onJoined: (community: CommunityInfo) => void;
}

type JoinStep = "input" | "joining" | "done" | "error";

export function JoinCommunityModal({ onClose, onJoined }: Props) {
  const [inviteLink, setInviteLink] = useState("");
  const [step, setStep] = useState<JoinStep>("input");
  const [statusText, setStatusText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);

  const parseInvite = (link: string): string | null => {
    const trimmed = link.trim();
    // Accept bitcord://join/<base64url> or raw base64url
    if (trimmed.startsWith("bitcord://join/")) {
      return trimmed.slice("bitcord://join/".length);
    }
    // Accept raw base64url if it looks plausible (no spaces, reasonable length)
    if (/^[A-Za-z0-9_-]{10,}$/.test(trimmed)) {
      return trimmed;
    }
    return null;
  };

  const handleJoin = async () => {
    const invite = parseInvite(inviteLink);
    if (!invite) {
      setError("Invalid invite link. Expected format: bitcord://join/…");
      return;
    }
    setError(null);
    setStep("joining");
    setStatusText("Contacting DHT…");

    try {
      // Simulate progress messages during the async call
      const progressTimer = setTimeout(() => {
        setStatusText("Discovering peers…");
      }, 1200);

      const community = await rpcClient.communityJoin({ invite });
      clearTimeout(progressTimer);
      setStep("done");
      // Brief "done" flash before handing off
      setTimeout(() => onJoined(community), 600);
    } catch (err) {
      setStep("error");
      setError(err instanceof Error ? err.message : "Failed to join community");
    }
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape" && step !== "joining") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [onClose, step]);

  const inputStyle: React.CSSProperties = {
    width: "100%",
    padding: "0.5rem 0.75rem",
    background: "var(--color-bc-surface-3)",
    border: "1px solid rgba(255,255,255,0.08)",
    borderRadius: "4px",
    color: "var(--color-bc-text)",
    fontSize: "0.9375rem",
    outline: "none",
    boxSizing: "border-box",
  };

  return (
    <div
      ref={overlayRef}
      onClick={(e) => { if (e.target === overlayRef.current && step !== "joining") onClose(); }}
      role="dialog"
      aria-modal="true"
      aria-label="Join Community"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.6)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
    >
      <div
        style={{
          background: "var(--color-bc-surface-2)",
          borderRadius: "8px",
          width: "420px",
          maxWidth: "calc(100vw - 2rem)",
          padding: "1.5rem",
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.25rem" }}>
          <h2 style={{ margin: 0, fontSize: "1.25rem", fontWeight: 700, color: "var(--color-bc-text)" }}>
            Join Community
          </h2>
          <button
            onClick={onClose}
            disabled={step === "joining"}
            aria-label="Close dialog"
            style={{ background: "none", border: "none", color: "var(--color-bc-muted)", cursor: step === "joining" ? "not-allowed" : "pointer", padding: "2px", display: "flex" }}
          >
            <X size={20} />
          </button>
        </div>

        {(step === "input" || step === "error") && (
          <>
            <label style={{ display: "block", marginBottom: "1rem" }}>
              <span style={{ display: "block", fontSize: "0.75rem", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", color: "var(--color-bc-muted)", marginBottom: "0.375rem" }}>
                Invite Link
              </span>
              <input
                type="text"
                value={inviteLink}
                onChange={(e) => { setInviteLink(e.target.value); setError(null); }}
                onKeyDown={(e) => { if (e.key === "Enter") void handleJoin(); }}
                placeholder="bitcord://join/…"
                autoFocus
                style={inputStyle}
              />
              <span style={{ display: "block", fontSize: "0.8125rem", color: "var(--color-bc-muted)", marginTop: "0.375rem" }}>
                Paste an invite link shared by a community member.
              </span>
            </label>

            {error && (
              <p style={{ color: "var(--color-bc-danger)", fontSize: "0.875rem", margin: "0 0 1rem" }}>
                {error}
              </p>
            )}

            <div style={{ display: "flex", gap: "0.75rem", justifyContent: "flex-end" }}>
              <button
                type="button"
                onClick={onClose}
                style={{
                  padding: "0.5rem 1rem",
                  background: "transparent",
                  border: "1px solid rgba(255,255,255,0.12)",
                  borderRadius: "4px",
                  color: "var(--color-bc-text)",
                  cursor: "pointer",
                  fontSize: "0.9375rem",
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void handleJoin()}
                disabled={!inviteLink.trim()}
                style={{
                  padding: "0.5rem 1rem",
                  background: !inviteLink.trim() ? "var(--color-bc-muted)" : "var(--color-bc-accent)",
                  border: "none",
                  borderRadius: "4px",
                  color: "#fff",
                  cursor: !inviteLink.trim() ? "not-allowed" : "pointer",
                  fontSize: "0.9375rem",
                  fontWeight: 600,
                }}
              >
                Join
              </button>
            </div>
          </>
        )}

        {step === "joining" && (
          <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "1rem", padding: "1rem 0" }}>
            <Loader2 size={36} style={{ color: "var(--color-bc-accent)", animation: "spin 1s linear infinite" }} />
            <p style={{ color: "var(--color-bc-text)", margin: 0 }}>{statusText}</p>
            <style>{`@keyframes spin { to { transform: rotate(360deg); } }`}</style>
          </div>
        )}

        {step === "done" && (
          <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: "0.75rem", padding: "1rem 0" }}>
            <span style={{ fontSize: "2rem" }}>✓</span>
            <p style={{ color: "var(--color-bc-success)", fontWeight: 600, margin: 0 }}>Joined successfully!</p>
          </div>
        )}
      </div>
    </div>
  );
}
