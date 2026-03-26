import React, { useState, useRef, useEffect } from "react";
import { X, WifiOff, ServerOff } from "lucide-react";
import { rpcClient } from "../hooks/useRpc";
import type { CommunityInfo } from "../lib/rpc-types";

interface Props {
  onClose: () => void;
  onCreated: (community: CommunityInfo) => void;
  serverEnabled?: boolean;
}

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

const labelTextStyle: React.CSSProperties = {
  display: "block",
  fontSize: "0.75rem",
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: "0.05em",
  color: "var(--color-bc-muted)",
  marginBottom: "0.375rem",
};

export function CreateCommunityModal({ onClose, onCreated, serverEnabled = true }: Props) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [seedNode, setSeedNode] = useState("");
  const [seedFingerprint, setSeedFingerprint] = useState("");
  const [selfHost, setSelfHost] = useState(false);
  const [hostingPassword, setHostingPassword] = useState("");
  const [localPublicAddr, setLocalPublicAddr] = useState<string | null>(null);
  const [localFingerprint, setLocalFingerprint] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    rpcClient.identityGet().then((info) => {
      if (info.public_addr) {
        setLocalPublicAddr(info.public_addr);
        if (serverEnabled) {
          setSelfHost(true);
          setSeedNode(info.public_addr);
        }
      }
      if (info.tls_fingerprint_hex) {
        setLocalFingerprint(info.tls_fingerprint_hex);
        if (serverEnabled) {
          setSeedFingerprint(info.tls_fingerprint_hex);
        }
      }
    });
  }, [serverEnabled]);

  const handleSelfHostChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const checked = e.target.checked;
    setSelfHost(checked);
    if (checked && localPublicAddr) {
      setSeedNode(localPublicAddr);
      setSeedFingerprint(localFingerprint ?? "");
    } else if (!checked) {
      setSeedNode("");
      setSeedFingerprint("");
    }
  };

  const seedNodes = seedNode.trim() ? [seedNode.trim()] : [];

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const community = await rpcClient.communityCreate({
        name: name.trim(),
        description: description.trim(),
        seed_nodes: seedNodes,
        seed_fingerprint_hex: seedNodes.length > 0 ? (seedFingerprint.trim() || null) : null,
        hosting_password: hostingPassword.trim() || null,
      });
      onCreated(community);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create community");
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
      aria-label="Create Community"
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
          width: "440px",
          maxWidth: "calc(100vw - 2rem)",
          maxHeight: "calc(100vh - 4rem)",
          overflowY: "auto",
          padding: "1.5rem",
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.25rem" }}>
          <h2 style={{ margin: 0, fontSize: "1.25rem", fontWeight: 700, color: "var(--color-bc-text)" }}>
            Create Community
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
          <label style={{ display: "block", marginBottom: "1rem" }}>
            <span style={labelTextStyle}>
              Community Name <span style={{ color: "var(--color-bc-danger)" }}>*</span>
            </span>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. My Community"
              maxLength={100}
              required
              autoFocus
              style={inputStyle}
            />
          </label>

          <label style={{ display: "block", marginBottom: "1rem" }}>
            <span style={labelTextStyle}>Description</span>
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Optional description"
              rows={3}
              maxLength={500}
              style={{ ...inputStyle, resize: "vertical" }}
            />
          </label>

          {!serverEnabled && (
            <div
              role="alert"
              style={{
                display: "flex",
                gap: "0.625rem",
                alignItems: "flex-start",
                padding: "0.625rem 0.75rem",
                marginBottom: "1.25rem",
                background: "rgba(99, 102, 241, 0.1)",
                border: "1px solid rgba(99, 102, 241, 0.35)",
                borderRadius: "6px",
                color: "var(--color-bc-text)",
                fontSize: "0.8125rem",
                lineHeight: 1.45,
              }}
            >
              <ServerOff size={15} style={{ flexShrink: 0, marginTop: "1px", color: "#818cf8" }} />
              <span>
                <strong style={{ color: "#818cf8" }}>Embedded server is disabled.</strong>{" "}
                Self-hosting is not available. You must specify an external seed node so peers can reach this community. If you want to self-host, enable the embedded server in settings and restart the app.
              </span>
            </div>
          )}

          {serverEnabled && localPublicAddr && (
            <label style={{
              display: "flex",
              alignItems: "center",
              gap: "0.625rem",
              marginBottom: "1.25rem",
              cursor: "pointer",
              padding: "0.75rem",
              background: "var(--color-bc-surface-3)",
              borderRadius: "6px",
              border: "1px solid rgba(255,255,255,0.04)"
            }}>
              <input
                type="checkbox"
                checked={selfHost}
                onChange={handleSelfHostChange}
                style={{ width: "16px", height: "16px", cursor: "pointer" }}
              />
              <div style={{ display: "flex", flexDirection: "column" }}>
                <span style={{ fontSize: "0.875rem", fontWeight: 600, color: "var(--color-bc-text)" }}>
                  Self-host on this device
                </span>
                <span style={{ fontSize: "0.75rem", color: "var(--color-bc-muted)" }}>
                  Use your public IP ({localPublicAddr}) as the seed node
                </span>
              </div>
            </label>
          )}

          <label style={{ display: "block", marginBottom: "1rem" }}>
            <span style={labelTextStyle}>
              Seed Node Address{serverEnabled ? " (optional)" : <> <span style={{ color: "var(--color-bc-danger)" }}>*</span></>}
            </span>
            <input
              type="text"
              value={seedNode}
              onChange={(e) => { setSeedNode(e.target.value); setSelfHost(false); setSeedFingerprint(""); }}
              placeholder="e.g. 1.2.3.4:9042"
              style={inputStyle}
            />
            <p style={{ margin: "0.25rem 0 0", fontSize: "0.75rem", color: "var(--color-bc-muted)" }}>
              The always-on node that hosts this community for peers to connect to.
            </p>
          </label>

          {seedNodes.length > 0 && (
            <label style={{ display: "block", marginBottom: "1.25rem" }}>
              <span style={labelTextStyle}>
                Seed Node Fingerprint <span style={{ color: "var(--color-bc-danger)" }}>*</span>
              </span>
              <input
                type="text"
                value={seedFingerprint}
                onChange={(e) => setSeedFingerprint(e.target.value)}
                placeholder="64-char hex SHA-256 of the seed node's TLS certificate"
                maxLength={64}
                style={{ ...inputStyle, fontFamily: "monospace", fontSize: "0.8125rem" }}
              />
              <p style={{ margin: "0.25rem 0 0", fontSize: "0.75rem", color: "var(--color-bc-muted)" }}>
                Shown on the seed node's console at startup. Prevents connecting to an impostor.
              </p>
            </label>
          )}

          <div style={{ marginBottom: "1.25rem" }}>
            <span style={labelTextStyle}>Hosting Password (optional)</span>
            <input
              type="password"
              value={hostingPassword}
              onChange={(e) => setHostingPassword(e.target.value)}
              placeholder="Password for private hosting node"
              style={inputStyle}
            />
            <p style={{ margin: "0.25rem 0 0", fontSize: "0.75rem", color: "var(--color-bc-muted)" }}>
              When set, the seed node must provide this password to host the community.
            </p>
          </div>

          {seedNodes.length === 0 && serverEnabled && (
            <div
              role="alert"
              style={{
                display: "flex",
                gap: "0.625rem",
                alignItems: "flex-start",
                padding: "0.625rem 0.75rem",
                marginBottom: "1rem",
                background: "rgba(250, 173, 20, 0.1)",
                border: "1px solid rgba(250, 173, 20, 0.35)",
                borderRadius: "6px",
                color: "var(--color-bc-text)",
                fontSize: "0.8125rem",
                lineHeight: 1.45,
              }}
            >
              <WifiOff size={15} style={{ flexShrink: 0, marginTop: "1px", color: "#faad14" }} />
              <span>
                <strong style={{ color: "#faad14" }}>Local-only community.</strong>{" "}
                Without a seed node, this community is only reachable while your app is open.
                Remote peers cannot join unless you add a seed node address.
              </span>
            </div>
          )}

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
              disabled={loading || !name.trim() || (!serverEnabled && seedNodes.length === 0) || (seedNodes.length > 0 && seedFingerprint.trim().length !== 64)}
              style={{
                padding: "0.5rem 1rem",
                background: loading || !name.trim() || (!serverEnabled && seedNodes.length === 0) || (seedNodes.length > 0 && seedFingerprint.trim().length !== 64) ? "var(--color-bc-muted)" : "var(--color-bc-accent)",
                border: "none",
                borderRadius: "4px",
                color: "#fff",
                cursor: loading || !name.trim() || (!serverEnabled && seedNodes.length === 0) || (seedNodes.length > 0 && seedFingerprint.trim().length !== 64) ? "not-allowed" : "pointer",
                fontSize: "0.9375rem",
                fontWeight: 600,
              }}
            >
              {loading ? "Creating…" : "Create Community"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
