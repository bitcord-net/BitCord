import { useState, useEffect, type CSSProperties, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { rpcClient } from "../hooks/useRpc";

const S = {
  page: {
    height: "100%",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: "var(--color-bc-base)",
    padding: "1rem",
  } as CSSProperties,

  card: {
    background: "var(--color-bc-surface-2)",
    borderRadius: "8px",
    padding: "2.5rem",
    width: "100%",
    maxWidth: "420px",
    color: "var(--color-bc-text)",
  } as CSSProperties,

  logo: {
    fontSize: "1.75rem",
    fontWeight: 700,
    marginBottom: "0.25rem",
    color: "var(--color-bc-text)",
  } as CSSProperties,

  subtitle: {
    color: "var(--color-bc-muted)",
    marginBottom: "1.75rem",
    fontSize: "0.9375rem",
  } as CSSProperties,

  label: {
    display: "block",
    fontWeight: 600,
    fontSize: "0.75rem",
    letterSpacing: "0.05em",
    textTransform: "uppercase" as const,
    color: "var(--color-bc-muted)",
    marginBottom: "0.5rem",
  } as CSSProperties,

  input: {
    display: "block",
    width: "100%",
    background: "var(--color-bc-surface-3)",
    border: "1px solid rgba(255,255,255,0.07)",
    borderRadius: "4px",
    padding: "0.625rem 0.75rem",
    color: "var(--color-bc-text)",
    fontSize: "1rem",
    outline: "none",
    transition: "border-color 0.15s",
  } as CSSProperties,

  error: {
    color: "var(--color-bc-danger)",
    fontSize: "0.875rem",
    marginTop: "0.5rem",
  } as CSSProperties,

  btn: {
    display: "block",
    width: "100%",
    marginTop: "1.5rem",
    padding: "0.75rem",
    background: "var(--color-bc-accent)",
    color: "#fff",
    border: "none",
    borderRadius: "4px",
    fontSize: "0.9375rem",
    fontWeight: 600,
    cursor: "pointer",
    transition: "background 0.15s",
  } as CSSProperties,

  checkboxRow: {
    display: "flex",
    alignItems: "center",
    gap: "0.5rem",
    marginTop: "1rem",
    fontSize: "0.875rem",
    color: "var(--color-bc-muted)",
  } as CSSProperties,
};

export function UnlockPage() {
  const navigate = useNavigate();
  const [passphrase, setPassphrase] = useState("");
  const [savePassphrase, setSavePassphrase] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [unlocking, setUnlocking] = useState(false);

  useEffect(() => {
    invoke<{ save_passphrase_enabled: boolean }>("get_backend_status")
      .then((status) => {
        setSavePassphrase(status.save_passphrase_enabled);
      })
      .catch(() => {});
  }, []);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!passphrase) {
      setError("Please enter your passphrase.");
      return;
    }
    setError(null);
    setUnlocking(true);
    try {
      await invoke("unlock_identity", {
        passphrase,
        savePassphrase,
      });

      rpcClient.forceReconnect();

      // Backend will emit backend_status { status: "ready" } which triggers
      // navigation via the App-level listener (ProtectedRoute).
      navigate("/app", { replace: true });
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : typeof err === "string"
          ? err
          : "Wrong passphrase or corrupt identity file."
      );
      setUnlocking(false);
    }
  };

  return (
    <div style={S.page} role="main">
      <div style={S.card}>
        <h1 style={S.logo}>BitCord</h1>
        <p style={S.subtitle}>
          Enter your passphrase to unlock your identity.
        </p>

        <form onSubmit={handleSubmit} noValidate>
          <label htmlFor="unlock-passphrase" style={S.label}>
            Passphrase
          </label>
          <input
            id="unlock-passphrase"
            type="password"
            value={passphrase}
            onChange={(e) => {
              setPassphrase(e.target.value);
              setError(null);
            }}
            placeholder="Enter your passphrase"
            autoFocus
            autoComplete="current-password"
            style={S.input}
            disabled={unlocking}
          />

          {error && (
            <p style={S.error} role="alert">
              {error}
            </p>
          )}

          <label style={S.checkboxRow}>
            <input
              type="checkbox"
              checked={savePassphrase}
              onChange={(e) => setSavePassphrase(e.target.checked)}
              disabled={unlocking}
            />
            Save passphrase (uses OS keychain)
          </label>

          <button type="submit" style={S.btn} disabled={unlocking}>
            {unlocking ? "Unlocking…" : "Unlock"}
          </button>
        </form>
      </div>
    </div>
  );
}
