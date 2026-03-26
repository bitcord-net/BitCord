import { useState, useEffect, useCallback, useRef, type CSSProperties, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { useRpc, rpcClient } from "../hooks/useRpc";
import { useIdentityStore } from "../store/identity";
import { invoke } from "@tauri-apps/api/core";

// ── Passphrase strength ───────────────────────────────────────────────────────

function measureStrength(p: string): 0 | 1 | 2 | 3 | 4 {
  if (p.length === 0) return 0;
  let score = 0;
  if (p.length >= 8) score++;
  if (p.length >= 14) score++;
  if (/[A-Z]/.test(p) && /[a-z]/.test(p)) score++;
  if (/[0-9]/.test(p)) score++;
  if (/[^A-Za-z0-9]/.test(p)) score++;
  return Math.min(4, score) as 0 | 1 | 2 | 3 | 4;
}

const STRENGTH_LABEL = ["Too short", "Weak", "Fair", "Strong", "Very strong"];
const STRENGTH_COLOR = ["#ed4245", "#ed4245", "#faa61a", "#3ba55c", "#3ba55c"];

// ── Shared UI primitives ──────────────────────────────────────────────────────

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
    maxWidth: "460px",
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

  btnSecondary: {
    background: "var(--color-bc-surface-3)",
    color: "var(--color-bc-text)",
  } as CSSProperties,

  stepIndicator: {
    display: "flex",
    gap: "0.375rem",
    marginBottom: "2rem",
  } as CSSProperties,
};

function StepDot({ active, done }: { active: boolean; done: boolean }) {
  return (
    <span
      style={{
        flex: 1,
        height: "3px",
        borderRadius: "2px",
        background: done
          ? "var(--color-bc-accent)"
          : active
          ? "var(--color-bc-accent)"
          : "var(--color-bc-surface-3)",
        opacity: active ? 1 : done ? 0.8 : 0.4,
        transition: "background 0.3s, opacity 0.3s",
      }}
    />
  );
}

// ── Steps ─────────────────────────────────────────────────────────────────────

type Step = 1 | 2 | 3 | 4;

export function OnboardingPage() {
  const navigate = useNavigate();
  const { client, isConnected } = useRpc();

  const [step, setStep] = useState<Step>(1);
  const [displayName, setDisplayName] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [confirmPassphrase, setConfirmPassphrase] = useState("");
  const [savePassphrase, setSavePassphrase] = useState(false);
  const [peerId, setPeerId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const generatingRef = useRef(false);

  const strength = measureStrength(passphrase);

  // Step 1 — display name
  const handleNameSubmit = (e: FormEvent) => {
    e.preventDefault();
    const name = displayName.trim();
    if (name.length < 2) {
      setError("Display name must be at least 2 characters.");
      return;
    }
    if (name.length > 32) {
      setError("Display name must be 32 characters or fewer.");
      return;
    }
    setError(null);
    setStep(2);
  };

  // Step 2 — passphrase
  const handlePassphraseSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (passphrase !== confirmPassphrase) {
      setError("Passphrases do not match.");
      return;
    }
    if (strength < 2) {
      setError("Please choose a stronger passphrase.");
      return;
    }
    setError(null);
    setStep(3);
  };

  // Step 3 — generate identity
  const generate = useCallback(async () => {
    if (generatingRef.current) return;
    generatingRef.current = true;
    try {
      // Create identity with the real passphrase via Tauri command.
      // This saves the keystore encrypted and starts the backend.
      await invoke("create_identity", {
        passphrase,
        displayName: displayName.trim(),
        savePassphrase,
      });

      // Backend has started; force the RPC client to connect immediately
      // instead of waiting for its next poll/reconnect interval.
      rpcClient.forceReconnect();

      // Wait for the RPC server to come up, then load identity.
      const waitForRpc = async () => {
        for (let i = 0; i < 50; i++) {
          if (isConnected) break;
          await new Promise((r) => setTimeout(r, 200));
        }
      };
      await waitForRpc();
      const identity = await client.identityGet();
      setPeerId(identity.peer_id);
      useIdentityStore.setState({ identity, isLoaded: true });
      setStep(4);
    } catch (e) {
      setError(e instanceof Error ? e.message : typeof e === "string" ? e : "Failed to set up identity.");
      setStep(2);
    } finally {
      generatingRef.current = false;
    }
  }, [client, displayName, passphrase, savePassphrase, isConnected]);

  useEffect(() => {
    if (step === 3) {
      void generate();
    }
  }, [step, generate]);

  const copyPeerId = async () => {
    await navigator.clipboard.writeText(peerId);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleFinish = () => navigate("/app", { replace: true });

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <div style={S.page} role="main">
      <div style={S.card}>
        <h1 style={S.logo}>BitCord</h1>

        {/* Step indicators */}
        <div style={S.stepIndicator} aria-hidden="true">
          {([1, 2, 3, 4] as Step[]).map((n) => (
            <StepDot key={n} active={step === n} done={step > n} />
          ))}
        </div>

        {/* Step 1 — Display Name */}
        {step === 1 && (
          <form onSubmit={handleNameSubmit} noValidate>
            <p style={S.subtitle}>
              Choose a display name to get started. You can change it later.
            </p>
            <label htmlFor="display-name" style={S.label}>
              Display name
            </label>
            <input
              id="display-name"
              type="text"
              value={displayName}
              onChange={(e) => {
                setDisplayName(e.target.value);
                setError(null);
              }}
              placeholder="e.g. Alice"
              autoFocus
              autoComplete="nickname"
              maxLength={32}
              style={S.input}
              aria-required="true"
              aria-describedby={error ? "name-error" : undefined}
            />
            {error && (
              <p id="name-error" style={S.error} role="alert">
                {error}
              </p>
            )}
            <button type="submit" style={S.btn}>
              Continue
            </button>
          </form>
        )}

        {/* Step 2 — Passphrase */}
        {step === 2 && (
          <form onSubmit={handlePassphraseSubmit} noValidate>
            <p style={S.subtitle}>
              Your passphrase encrypts your identity on disk. Choose something
              memorable and strong.
            </p>

            <label htmlFor="passphrase" style={S.label}>
              Passphrase
            </label>
            <input
              id="passphrase"
              type="password"
              value={passphrase}
              onChange={(e) => {
                setPassphrase(e.target.value);
                setError(null);
              }}
              placeholder="Enter a strong passphrase"
              autoFocus
              autoComplete="new-password"
              style={S.input}
              aria-required="true"
            />

            {/* Strength meter */}
            {passphrase.length > 0 && (
              <div
                style={{ marginTop: "0.5rem" }}
                aria-label={`Passphrase strength: ${STRENGTH_LABEL[strength]}`}
              >
                <div
                  style={{
                    display: "flex",
                    gap: "4px",
                    marginBottom: "4px",
                  }}
                  aria-hidden="true"
                >
                  {[1, 2, 3, 4].map((i) => (
                    <div
                      key={i}
                      style={{
                        flex: 1,
                        height: "4px",
                        borderRadius: "2px",
                        background:
                          i <= strength
                            ? STRENGTH_COLOR[strength]
                            : "var(--color-bc-surface-3)",
                        transition: "background 0.2s",
                      }}
                    />
                  ))}
                </div>
                <span
                  style={{
                    fontSize: "0.8125rem",
                    color: STRENGTH_COLOR[strength],
                  }}
                >
                  {STRENGTH_LABEL[strength]}
                </span>
              </div>
            )}

            <label
              htmlFor="confirm-passphrase"
              style={{ ...S.label, marginTop: "1rem" }}
            >
              Confirm passphrase
            </label>
            <input
              id="confirm-passphrase"
              type="password"
              value={confirmPassphrase}
              onChange={(e) => {
                setConfirmPassphrase(e.target.value);
                setError(null);
              }}
              placeholder="Repeat your passphrase"
              autoComplete="new-password"
              style={S.input}
              aria-required="true"
              aria-describedby={error ? "pp-error" : undefined}
            />
            {error && (
              <p id="pp-error" style={S.error} role="alert">
                {error}
              </p>
            )}

            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.5rem",
                marginTop: "1rem",
                fontSize: "0.875rem",
                color: "var(--color-bc-muted)",
              }}
            >
              <input
                type="checkbox"
                checked={savePassphrase}
                onChange={(e) => setSavePassphrase(e.target.checked)}
              />
              Save passphrase (uses OS keychain)
            </label>

            <div style={{ display: "flex", gap: "0.75rem" }}>
              <button
                type="button"
                onClick={() => setStep(1)}
                style={{ ...S.btn, ...S.btnSecondary }}
              >
                Back
              </button>
              <button type="submit" style={S.btn}>
                Continue
              </button>
            </div>
          </form>
        )}

        {/* Step 3 — Generating */}
        {step === 3 && (
          <div
            style={{ textAlign: "center", padding: "1rem 0" }}
            aria-live="polite"
          >
            <p style={{ ...S.subtitle, marginBottom: "2rem" }}>
              Setting up your identity…
            </p>
            {/* Spinner animation via CSS */}
            <div
              style={{
                width: "48px",
                height: "48px",
                borderRadius: "50%",
                border: "4px solid var(--color-bc-surface-3)",
                borderTopColor: "var(--color-bc-accent)",
                margin: "0 auto 1.5rem",
                animation: "bc-spin 0.8s linear infinite",
              }}
              aria-hidden="true"
            />
            <style>{`@keyframes bc-spin { to { transform: rotate(360deg); } }`}</style>
            {error && (
              <p style={S.error} role="alert">
                {error}
              </p>
            )}
          </div>
        )}

        {/* Step 4 — Done */}
        {step === 4 && (
          <div>
            <p style={S.subtitle}>
              Your decentralized identity is ready. Share your Peer ID to let
              others connect with you directly.
            </p>

            <label style={S.label}>Your Peer ID</label>
            <div
              style={{
                background: "var(--color-bc-surface-3)",
                borderRadius: "4px",
                padding: "0.625rem 0.75rem",
                fontFamily: "monospace",
                fontSize: "0.8125rem",
                wordBreak: "break-all",
                color: "var(--color-bc-text)",
                userSelect: "all",
              }}
              aria-label="Your Peer ID"
            >
              {peerId}
            </div>

            <button
              type="button"
              onClick={copyPeerId}
              style={{
                ...S.btn,
                background: copied
                  ? "var(--color-bc-success)"
                  : "var(--color-bc-surface-3)",
                color: "var(--color-bc-text)",
                marginTop: "0.75rem",
              }}
              aria-label="Copy Peer ID to clipboard"
            >
              {copied ? "Copied!" : "Copy Peer ID"}
            </button>

            <button
              type="button"
              onClick={handleFinish}
              style={S.btn}
              autoFocus
            >
              Enter BitCord
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
