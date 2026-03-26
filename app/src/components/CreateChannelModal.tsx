import React, { useState, useRef, useEffect } from "react";
import { X } from "lucide-react";
import { rpcClient } from "../hooks/useRpc";
import type { ChannelInfo, ChannelKind } from "../lib/rpc-types";

interface Props {
  communityId: string;
  initialKind?: ChannelKind;
  onClose: () => void;
  onCreated: (channel: ChannelInfo) => void;
}

const KINDS: { value: ChannelKind; label: string; description: string }[] = [
  { value: "text", label: "Text", description: "Send messages, images, and files" },
  { value: "announcement", label: "Announcement", description: "Read-only broadcast channel" },
  { value: "voice", label: "Voice", description: "Voice and video chat" },
];

export function CreateChannelModal({ communityId, initialKind = "text", onClose, onCreated }: Props) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<ChannelKind>(initialKind);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const channel = await rpcClient.channelCreate({
        community_id: communityId,
        name: name.trim().toLowerCase().replace(/\s+/g, "-"),
        kind,
      });
      onCreated(channel);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create channel");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div
      ref={overlayRef}
      onClick={(e) => { if (e.target === overlayRef.current) onClose(); }}
      role="dialog"
      aria-modal="true"
      aria-label="Create Channel"
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
          width: "400px",
          maxWidth: "calc(100vw - 2rem)",
          padding: "1.5rem",
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.25rem" }}>
          <h2 style={{ margin: 0, fontSize: "1.25rem", fontWeight: 700, color: "var(--color-bc-text)" }}>
            Create Channel
          </h2>
          <button
            onClick={onClose}
            aria-label="Close dialog"
            style={{ background: "none", border: "none", color: "var(--color-bc-muted)", cursor: "pointer", padding: "2px", display: "flex" }}
          >
            <X size={20} />
          </button>
        </div>

        <form onSubmit={handleSubmit}>
          {/* Channel type */}
          <div style={{ marginBottom: "1.25rem" }}>
            <span style={{ display: "block", fontSize: "0.75rem", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", color: "var(--color-bc-muted)", marginBottom: "0.5rem" }}>
              Channel Type
            </span>
            <div style={{ display: "flex", flexDirection: "column", gap: "0.375rem" }}>
              {KINDS.map((k) => (
                <label
                  key={k.value}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: "0.75rem",
                    padding: "0.625rem 0.75rem",
                    background: kind === k.value ? "var(--color-bc-surface-hover)" : "var(--color-bc-surface-3)",
                    borderRadius: "4px",
                    cursor: "pointer",
                    border: kind === k.value ? "1px solid var(--color-bc-accent)" : "1px solid transparent",
                  }}
                >
                  <input
                    type="radio"
                    name="kind"
                    value={k.value}
                    checked={kind === k.value}
                    onChange={() => setKind(k.value)}
                    style={{ accentColor: "var(--color-bc-accent)" }}
                  />
                  <div>
                    <div style={{ fontWeight: 600, color: "var(--color-bc-text)", fontSize: "0.9375rem" }}>{k.label}</div>
                    <div style={{ fontSize: "0.8125rem", color: "var(--color-bc-muted)" }}>{k.description}</div>
                  </div>
                </label>
              ))}
            </div>
          </div>

          {/* Channel name */}
          <label style={{ display: "block", marginBottom: "1.25rem" }}>
            <span style={{ display: "block", fontSize: "0.75rem", fontWeight: 600, textTransform: "uppercase", letterSpacing: "0.05em", color: "var(--color-bc-muted)", marginBottom: "0.375rem" }}>
              Channel Name <span style={{ color: "var(--color-bc-danger)" }}>*</span>
            </span>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="new-channel"
              maxLength={100}
              required
              autoFocus
              style={{
                width: "100%",
                padding: "0.5rem 0.75rem",
                background: "var(--color-bc-surface-3)",
                border: "1px solid rgba(255,255,255,0.08)",
                borderRadius: "4px",
                color: "var(--color-bc-text)",
                fontSize: "0.9375rem",
                outline: "none",
                boxSizing: "border-box",
              }}
            />
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
              type="submit"
              disabled={loading || !name.trim()}
              style={{
                padding: "0.5rem 1rem",
                background: loading || !name.trim() ? "var(--color-bc-muted)" : "var(--color-bc-accent)",
                border: "none",
                borderRadius: "4px",
                color: "#fff",
                cursor: loading || !name.trim() ? "not-allowed" : "pointer",
                fontSize: "0.9375rem",
                fontWeight: 600,
              }}
            >
              {loading ? "Creating…" : "Create Channel"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
