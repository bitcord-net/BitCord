import React, { useState, useEffect, useCallback } from "react";
import { useNavigate } from "react-router-dom";
import { X, User, Server, Palette, Bell, Wrench, Info, Copy, Check, Eye, EyeOff, Plus, Trash2, Mailbox } from "lucide-react";
import { useIdentityStore } from "../store/identity";
import { useSettingsStore, type Theme, type FontSize, type MessageDensity, type NotificationLevel } from "../store/settings";
import { rpcClient } from "../hooks/useRpc";
import { getLogText } from "../lib/logger";
import { useSubscription } from "../hooks/useSubscription";
import { invoke } from "@tauri-apps/api/core";
import type { NodeConfigDto, NodeMetricsSnapshot } from "../lib/rpc-types";

// ── Helpers ───────────────────────────────────────────────────────────────────

function useCopyText() {
  const [copied, setCopied] = useState(false);
  const copy = useCallback(async (text: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, []);
  return { copied, copy };
}

function SettingRow({ label, description, children }: { label: string; description?: string; children: React.ReactNode }) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "12px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
      <div>
        <div style={{ color: "var(--color-bc-text)", fontWeight: 500 }}>{label}</div>
        {description && <div style={{ color: "var(--color-bc-muted)", fontSize: 13, marginTop: 2 }}>{description}</div>}
      </div>
      <div style={{ flexShrink: 0, marginLeft: 24 }}>{children}</div>
    </div>
  );
}

function SectionHeader({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--color-bc-muted)", marginBottom: 8, marginTop: 24 }}>
      {children}
    </div>
  );
}

function Toggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      style={{
        width: 44, height: 24, borderRadius: 12, border: "none", cursor: "pointer",
        background: checked ? "var(--color-bc-accent)" : "var(--color-bc-surface-3)",
        position: "relative", transition: "background 0.2s", flexShrink: 0,
      }}
    >
      <span style={{
        position: "absolute", top: 3, left: checked ? 23 : 3, width: 18, height: 18,
        borderRadius: "50%", background: "white", transition: "left 0.2s",
      }} />
    </button>
  );
}

function Select<T extends string>({ value, onChange, options }: { value: T; onChange: (v: T) => void; options: { value: T; label: string }[] }) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value as T)}
      style={{
        background: "var(--color-bc-surface-3)", color: "var(--color-bc-text)", border: "1px solid var(--color-bc-surface-3)",
        borderRadius: 4, padding: "4px 8px", fontSize: 14, cursor: "pointer",
      }}
    >
      {options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
    </select>
  );
}

function TextInput({ value, onChange, placeholder, type = "text" }: { value: string; onChange: (v: string) => void; placeholder?: string; type?: string }) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      style={{
        background: "var(--color-bc-surface-2)", color: "var(--color-bc-text)", border: "1px solid var(--color-bc-surface-3)",
        borderRadius: 4, padding: "6px 10px", fontSize: 14, outline: "none", width: "100%",
      }}
    />
  );
}

function Btn({ onClick, children, variant = "primary", disabled }: { onClick?: () => void; children: React.ReactNode; variant?: "primary" | "danger" | "ghost"; disabled?: boolean }) {
  const bg = variant === "primary" ? "var(--color-bc-accent)" : variant === "danger" ? "var(--color-bc-danger)" : "var(--color-bc-surface-3)";
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      style={{
        background: bg, color: "white", border: "none", borderRadius: 4,
        padding: "8px 16px", fontSize: 14, cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.6 : 1, fontWeight: 500,
      }}
    >
      {children}
    </button>
  );
}

// ── Tab: Account ──────────────────────────────────────────────────────────────

function AccountTab() {
  const identity = useIdentityStore((s) => s.identity);
  const setDisplayName = useIdentityStore((s) => s.setDisplayName);
  const { copied, copy } = useCopyText();

  const [displayName, setDisplayNameLocal] = useState(identity?.display_name ?? "");
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState("");

  const [oldPass, setOldPass] = useState("");
  const [newPass, setNewPass] = useState("");
  const [confirmPass, setConfirmPass] = useState("");
  const [showOld, setShowOld] = useState(false);
  const [showNew, setShowNew] = useState(false);
  const [passError, setPassError] = useState("");
  const [passSaving, setPassSaving] = useState(false);
  const [savePassphrase, setSavePassphrase] = useState(false);
  const [showKeychainPrompt, setShowKeychainPrompt] = useState(false);
  const [keychainPass, setKeychainPass] = useState("");
  const [keychainError, setKeychainError] = useState("");

  useEffect(() => {
    invoke("get_save_passphrase").then((v) => setSavePassphrase(v as boolean)).catch(() => {});
  }, []);

  async function saveDisplayName() {
    if (!displayName.trim()) return;
    setSaving(true);
    try {
      await setDisplayName(displayName.trim());
      setSaveMsg("Saved!");
      setTimeout(() => setSaveMsg(""), 2000);
    } catch {
      setSaveMsg("Failed to save");
    } finally {
      setSaving(false);
    }
  }

  async function changePassphrase() {
    setPassError("");
    if (!oldPass || !newPass) { setPassError("All fields required"); return; }
    if (newPass !== confirmPass) { setPassError("Passphrases do not match"); return; }
    if (newPass.length < 8) { setPassError("Passphrase must be at least 8 characters"); return; }
    setPassSaving(true);
    try {
      await rpcClient.identityChangePassphrase({ old_passphrase: oldPass, new_passphrase: newPass });
      setOldPass(""); setNewPass(""); setConfirmPass("");
      setPassError("Passphrase changed successfully!");
      setTimeout(() => setPassError(""), 3000);
    } catch (e: unknown) {
      setPassError(e instanceof Error ? e.message : "Failed to change passphrase");
    } finally {
      setPassSaving(false);
    }
  }

  async function exportIdentity() {
    try {
      // Export as identity info JSON (no private keys)
      const data = JSON.stringify({ peer_id: identity?.peer_id, display_name: identity?.display_name, exported_at: new Date().toISOString() }, null, 2);
      const blob = new Blob([data], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `bitcord-identity-${Date.now()}.json`;
      a.click();
      URL.revokeObjectURL(url);
    } catch {
      // noop
    }
  }

  const passSuccess = passError === "Passphrase changed successfully!";

  return (
    <div>
      <SectionHeader>Profile</SectionHeader>

      <div style={{ marginBottom: 16 }}>
        <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--color-bc-muted)", marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.04em" }}>
          Display Name
        </label>
        <div style={{ display: "flex", gap: 8 }}>
          <TextInput value={displayName} onChange={setDisplayNameLocal} placeholder="Enter display name" />
          <Btn onClick={saveDisplayName} disabled={saving || !displayName.trim()}>
            {saveMsg || (saving ? "Saving…" : "Save")}
          </Btn>
        </div>
      </div>

      <div style={{ marginBottom: 8 }}>
        <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--color-bc-muted)", marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.04em" }}>
          Peer ID
        </label>
        <div style={{ display: "flex", alignItems: "center", gap: 8, background: "var(--color-bc-surface-2)", borderRadius: 4, padding: "8px 12px" }}>
          <span style={{ fontFamily: "monospace", fontSize: 13, color: "var(--color-bc-text)", flex: 1, wordBreak: "break-all" }}>
            {identity?.peer_id ?? "—"}
          </span>
          <button onClick={() => identity?.peer_id && copy(identity.peer_id)} aria-label="Copy Peer ID"
            style={{ background: "none", border: "none", cursor: "pointer", color: "var(--color-bc-muted)", display: "flex", alignItems: "center" }}>
            {copied ? <Check size={16} color="var(--color-bc-success)" /> : <Copy size={16} />}
          </button>
        </div>
      </div>

      <div style={{ marginBottom: 24 }}>
        <Btn variant="ghost" onClick={exportIdentity}>Export Identity Backup</Btn>
      </div>

      <SectionHeader>Change Passphrase</SectionHeader>

      <div style={{ display: "flex", flexDirection: "column", gap: 10, maxWidth: 400 }}>
        <div style={{ position: "relative" }}>
          <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--color-bc-muted)", marginBottom: 4, textTransform: "uppercase", letterSpacing: "0.04em" }}>
            Current Passphrase
          </label>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <TextInput type={showOld ? "text" : "password"} value={oldPass} onChange={setOldPass} placeholder="Current passphrase" />
            <button onClick={() => setShowOld((v) => !v)} style={{ background: "none", border: "none", cursor: "pointer", color: "var(--color-bc-muted)" }} aria-label="Toggle visibility">
              {showOld ? <EyeOff size={16} /> : <Eye size={16} />}
            </button>
          </div>
        </div>

        <div>
          <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--color-bc-muted)", marginBottom: 4, textTransform: "uppercase", letterSpacing: "0.04em" }}>
            New Passphrase
          </label>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <TextInput type={showNew ? "text" : "password"} value={newPass} onChange={setNewPass} placeholder="New passphrase (min 8 chars)" />
            <button onClick={() => setShowNew((v) => !v)} style={{ background: "none", border: "none", cursor: "pointer", color: "var(--color-bc-muted)" }} aria-label="Toggle visibility">
              {showNew ? <EyeOff size={16} /> : <Eye size={16} />}
            </button>
          </div>
          {newPass && (
            <div style={{ marginTop: 6, display: "flex", gap: 4 }}>
              {[1, 2, 3, 4].map((i) => {
                const strength = Math.min(4, Math.floor(newPass.length / 4) + (newPass.length >= 12 ? 1 : 0) + (/[A-Z]/.test(newPass) ? 1 : 0) + (/[0-9!@#$%]/.test(newPass) ? 1 : 0));
                const colors = ["var(--color-bc-danger)", "var(--color-bc-warning)", "var(--color-bc-warning)", "var(--color-bc-success)"];
                return <div key={i} style={{ flex: 1, height: 4, borderRadius: 2, background: i <= strength ? colors[Math.min(strength - 1, 3)] : "var(--color-bc-surface-3)" }} />;
              })}
            </div>
          )}
        </div>

        <div>
          <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--color-bc-muted)", marginBottom: 4, textTransform: "uppercase", letterSpacing: "0.04em" }}>
            Confirm New Passphrase
          </label>
          <TextInput type="password" value={confirmPass} onChange={setConfirmPass} placeholder="Confirm new passphrase" />
        </div>

        {passError && (
          <div style={{ fontSize: 13, color: passSuccess ? "var(--color-bc-success)" : "var(--color-bc-danger)", padding: "6px 10px", borderRadius: 4, background: passSuccess ? "rgba(59,165,92,0.1)" : "rgba(237,66,69,0.1)" }}>
            {passError}
          </div>
        )}

        <Btn onClick={changePassphrase} disabled={passSaving}>
          {passSaving ? "Changing…" : "Change Passphrase"}
        </Btn>
      </div>

      <SectionHeader>Security</SectionHeader>
      <SettingRow
        label="Save passphrase"
        description="Store passphrase in your OS keychain so you don't have to enter it on every launch."
      >
        <Toggle
          checked={savePassphrase}
          onChange={async (v) => {
            if (v) {
              setKeychainPass("");
              setKeychainError("");
              setShowKeychainPrompt(true);
            } else {
              await invoke("set_save_passphrase", { enabled: false, passphrase: "" });
              setSavePassphrase(false);
            }
          }}
        />
      </SettingRow>

      {showKeychainPrompt && (
        <div style={{ padding: "12px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
          <div style={{ fontSize: 13, color: "var(--color-bc-muted)", marginBottom: 8 }}>
            Enter your passphrase to save it to the OS keychain:
          </div>
          <input
            type="password"
            value={keychainPass}
            onChange={(e) => { setKeychainPass(e.target.value); setKeychainError(""); }}
            placeholder="Current passphrase"
            autoFocus
            style={{ display: "block", width: "100%", background: "var(--color-bc-surface-3)", border: "1px solid rgba(255,255,255,0.07)", borderRadius: 4, padding: "8px 10px", color: "var(--color-bc-text)", fontSize: 14, outline: "none" }}
          />
          {keychainError && <div style={{ color: "var(--color-bc-danger)", fontSize: 13, marginTop: 4 }}>{keychainError}</div>}
          <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
            <button
              onClick={async () => {
                try {
                  await invoke("set_save_passphrase", { enabled: true, passphrase: keychainPass });
                  setSavePassphrase(true);
                  setShowKeychainPrompt(false);
                } catch {
                  setKeychainError("Wrong passphrase.");
                }
              }}
              style={{ padding: "6px 16px", background: "var(--color-bc-accent)", color: "#fff", border: "none", borderRadius: 4, cursor: "pointer", fontWeight: 600, fontSize: 13 }}
            >
              Save
            </button>
            <button
              onClick={() => setShowKeychainPrompt(false)}
              style={{ padding: "6px 16px", background: "var(--color-bc-surface-3)", color: "var(--color-bc-text)", border: "none", borderRadius: 4, cursor: "pointer", fontSize: 13 }}
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      <DmMailboxSection />
    </div>
  );
}

// ── DM mailbox section (used inside AccountTab) ────────────────────────────────

function DmMailboxSection() {
  const [mailbox, setMailbox] = useState<string | null | undefined>(undefined);
  const [clearing, setClearing] = useState(false);

  useEffect(() => {
    rpcClient.nodeGetConfig()
      .then((c) => setMailbox(c.preferred_mailbox_node))
      .catch(() => setMailbox(null));
  }, []);

  const handleClear = async () => {
    setClearing(true);
    try {
      await rpcClient.dmClearPreferredMailbox();
      setMailbox(null);
    } catch {
      // noop — leave displayed value unchanged
    } finally {
      setClearing(false);
    }
  };

  return (
    <>
      <SectionHeader>Direct Messages</SectionHeader>
      <div style={{ padding: "12px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
          <Mailbox size={14} style={{ color: mailbox ? "var(--color-bc-success, #57f287)" : "var(--color-bc-muted)", flexShrink: 0 }} />
          <span style={{ color: "var(--color-bc-text)", fontWeight: 500, fontSize: 14 }}>
            Preferred Mailbox Node
          </span>
        </div>
        {mailbox === undefined ? (
          <span style={{ fontSize: 13, color: "var(--color-bc-muted)" }}>Loading…</span>
        ) : mailbox ? (
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ fontFamily: "monospace", fontSize: 13, color: "var(--color-bc-text)", flex: 1, wordBreak: "break-all" }}>
              {mailbox}
            </span>
            <button
              onClick={() => void handleClear()}
              disabled={clearing}
              style={{
                flexShrink: 0,
                padding: "4px 12px",
                background: "transparent",
                border: "1px solid rgba(255,255,255,0.12)",
                borderRadius: 4,
                color: "var(--color-bc-muted)",
                cursor: clearing ? "not-allowed" : "pointer",
                fontSize: 13,
                opacity: clearing ? 0.6 : 1,
              }}
            >
              {clearing ? "Clearing…" : "Clear"}
            </button>
          </div>
        ) : (
          <span style={{ fontSize: 13, color: "var(--color-bc-muted)" }}>
            Not set — mailbox node determined automatically by the DHT. Set one via a community's settings.
          </span>
        )}
        <p style={{ margin: "10px 0 0", fontSize: 12, color: "var(--color-bc-muted)", lineHeight: 1.5 }}>
          Clearing only stops re-announcing your preference — it does not send a retraction to the network. Existing DHT records on other nodes expire naturally within 24 hours. To override sooner, set a different community's node as your preferred mailbox.
        </p>
      </div>
    </>
  );
}

// ── Tab: Node ─────────────────────────────────────────────────────────────────

function NodeTab() {
  const [config, setConfig] = useState<NodeConfigDto | null>(null);
  const [metrics, setMetrics] = useState<NodeMetricsSnapshot | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState("");

  // local edits
  const [storageMb, setStorageMb] = useState(512);
  const [bandwidthKbps, setBandwidthKbps] = useState<number | null>(null);
  const [seedNodes, setSeedNodes] = useState<string[]>([]);
  const [newSeedNode, setNewSeedNode] = useState("");

  useEffect(() => {
    rpcClient.nodeGetConfig().then((c) => {
      setConfig(c);
      setStorageMb(c.storage_limit_mb);
      setBandwidthKbps(c.bandwidth_limit_kbps);
      setSeedNodes([...c.seed_nodes]);
    });
    rpcClient.nodeGetMetrics().then(setMetrics);
  }, []);

  useSubscription("node_metrics_updated", (e) => {
    setMetrics(e.data);
  });

  async function save() {
    setSaving(true);
    try {
      await rpcClient.nodeSetConfig({ storage_limit_mb: storageMb, bandwidth_limit_kbps: bandwidthKbps, seed_nodes: seedNodes });
      setSaveMsg("Saved!"); setTimeout(() => setSaveMsg(""), 2000);
    } catch {
      setSaveMsg("Failed to save");
    } finally {
      setSaving(false);
    }
  }

  function addSeedNode() {
    if (!newSeedNode.trim() || seedNodes.includes(newSeedNode.trim())) return;
    setSeedNodes([...seedNodes, newSeedNode.trim()]);
    setNewSeedNode("");
  }

  function removeSeedNode(addr: string) {
    setSeedNodes(seedNodes.filter((s) => s !== addr));
  }

  const GB = 1024;
  function fmtStorage(mb: number) {
    return mb >= GB ? `${(mb / GB).toFixed(1)} GiB` : `${mb} MiB`;
  }

  function fmtUptime(secs: number) {
    const h = Math.floor(secs / 3600); const m = Math.floor((secs % 3600) / 60);
    return h > 0 ? `${h}h ${m}m` : `${m}m`;
  }

  return (
    <div>
      {metrics && (
        <>
          <SectionHeader>Live Metrics</SectionHeader>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 12, marginBottom: 8 }}>
            {[
              { label: "Connected Peers", value: metrics.connected_peers },
              { label: "Stored Channels", value: metrics.stored_channels },
              { label: "Disk Usage", value: `${metrics.disk_usage_mb.toFixed(1)} MiB` },
              { label: "Bandwidth In", value: `${metrics.bandwidth_in_kbps.toFixed(1)} kbps` },
              { label: "Bandwidth Out", value: `${metrics.bandwidth_out_kbps.toFixed(1)} kbps` },
              { label: "Uptime", value: fmtUptime(metrics.uptime_secs) },
            ].map(({ label, value }) => (
              <div key={label} style={{ background: "var(--color-bc-surface-2)", borderRadius: 6, padding: "12px 14px" }}>
                <div style={{ fontSize: 11, color: "var(--color-bc-muted)", textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 4 }}>{label}</div>
                <div style={{ fontSize: 20, fontWeight: 700, color: "var(--color-bc-text)" }}>{value}</div>
              </div>
            ))}
          </div>
        </>
      )}

      {config && (
        <>
          <SectionHeader>Node Settings</SectionHeader>

          <div style={{ padding: "12px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
            <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 8 }}>
              <div>
                <div style={{ color: "var(--color-bc-text)", fontWeight: 500 }}>Storage Limit</div>
                <div style={{ color: "var(--color-bc-muted)", fontSize: 13 }}>Maximum disk space for channel history</div>
              </div>
              <div style={{ color: "var(--color-bc-accent)", fontWeight: 600 }}>{fmtStorage(storageMb)}</div>
            </div>
            <input
              type="range" min={100} max={50 * GB} step={100} value={storageMb}
              onChange={(e) => setStorageMb(Number(e.target.value))}
              style={{ width: "100%", accentColor: "var(--color-bc-accent)" }}
              aria-label="Storage limit"
            />
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: 11, color: "var(--color-bc-muted)", marginTop: 2 }}>
              <span>100 MiB</span><span>50 GiB</span>
            </div>
          </div>

          <SettingRow label="Bandwidth Limit" description="Limit upload/download speed (0 = unlimited)">
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <input
                type="number" min={0} step={128}
                value={bandwidthKbps ?? 0}
                onChange={(e) => { const v = Number(e.target.value); setBandwidthKbps(v === 0 ? null : v); }}
                style={{ width: 90, background: "var(--color-bc-surface-2)", color: "var(--color-bc-text)", border: "1px solid var(--color-bc-surface-3)", borderRadius: 4, padding: "4px 8px", fontSize: 14 }}
                aria-label="Bandwidth limit kbps"
              />
              <span style={{ fontSize: 13, color: "var(--color-bc-muted)" }}>kbps</span>
            </div>
          </SettingRow>

          <div style={{ padding: "12px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
            <div style={{ color: "var(--color-bc-text)", fontWeight: 500, marginBottom: 8 }}>Seed Node Addresses</div>
            <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
              <div style={{ flex: 1 }}>
                <TextInput value={newSeedNode} onChange={setNewSeedNode} placeholder="e.g. 1.2.3.4:7332" />
              </div>
              <Btn onClick={addSeedNode} variant="ghost"><Plus size={14} /></Btn>
            </div>
            {seedNodes.length === 0 && <div style={{ color: "var(--color-bc-muted)", fontSize: 13 }}>No seed nodes configured</div>}
            {seedNodes.map((addr) => (
              <div key={addr} style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4, background: "var(--color-bc-surface-2)", borderRadius: 4, padding: "4px 10px" }}>
                <span style={{ flex: 1, fontFamily: "monospace", fontSize: 12, wordBreak: "break-all" }}>{addr}</span>
                <button onClick={() => removeSeedNode(addr)} style={{ background: "none", border: "none", cursor: "pointer", color: "var(--color-bc-muted)" }} aria-label="Remove seed node">
                  <Trash2 size={13} />
                </button>
              </div>
            ))}
          </div>

          <div style={{ marginTop: 16, display: "flex", gap: 8, alignItems: "center" }}>
            <Btn onClick={save} disabled={saving}>{saving ? "Saving…" : "Save Node Settings"}</Btn>
            {saveMsg && <span style={{ fontSize: 13, color: saveMsg === "Saved!" ? "var(--color-bc-success)" : "var(--color-bc-danger)" }}>{saveMsg}</span>}
          </div>
        </>
      )}
    </div>
  );
}

// ── Tab: Appearance ───────────────────────────────────────────────────────────

function AppearanceTab() {
  const { theme, fontSize, messageDensity, animatedEmoji, setTheme, setFontSize, setMessageDensity, setAnimatedEmoji } = useSettingsStore();

  return (
    <div>
      <SectionHeader>Theme</SectionHeader>
      <div style={{ display: "flex", gap: 10, marginBottom: 8 }}>
        {(["dark", "light", "system"] as Theme[]).map((t) => (
          <button
            key={t}
            onClick={() => setTheme(t)}
            style={{
              flex: 1, padding: "12px 8px", borderRadius: 6, cursor: "pointer",
              border: `2px solid ${theme === t ? "var(--color-bc-accent)" : "var(--color-bc-surface-3)"}`,
              background: t === "light" ? "#f2f3f5" : t === "dark" ? "#0b0d0f" : "var(--color-bc-surface-2)",
              color: t === "light" ? "#2e3338" : "var(--color-bc-text)",
              fontWeight: 500, fontSize: 14,
            }}
          >
            {t.charAt(0).toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>

      <SectionHeader>Text</SectionHeader>
      <SettingRow label="Font Size">
        <Select<FontSize> value={fontSize} onChange={setFontSize} options={[
          { value: "small", label: "Small (13px)" },
          { value: "medium", label: "Medium (15px)" },
          { value: "large", label: "Large (17px)" },
        ]} />
      </SettingRow>

      <SectionHeader>Messages</SectionHeader>
      <SettingRow label="Message Density" description="Controls spacing between messages">
        <Select<MessageDensity> value={messageDensity} onChange={setMessageDensity} options={[
          { value: "compact", label: "Compact" },
          { value: "cozy", label: "Cozy" },
          { value: "comfortable", label: "Comfortable" },
        ]} />
      </SettingRow>
      <SettingRow label="Animated Emoji" description="Play emoji animations in messages">
        <Toggle checked={animatedEmoji} onChange={setAnimatedEmoji} />
      </SettingRow>
    </div>
  );
}

// ── Tab: Notifications ────────────────────────────────────────────────────────

function NotificationsTab() {
  const {
    defaultNotificationLevel, setDefaultNotificationLevel,
    osNotificationsEnabled, setOsNotificationsEnabled,
    soundEnabled, setSoundEnabled,
    dndSchedule, setDndSchedule,
  } = useSettingsStore();

  function fmtHour(h: number) {
    const ampm = h >= 12 ? "PM" : "AM";
    const h12 = h % 12 || 12;
    return `${h12}:00 ${ampm}`;
  }

  return (
    <div>
      <SectionHeader>Default Notification Level</SectionHeader>
      <div style={{ display: "flex", gap: 10, marginBottom: 16 }}>
        {(["all", "mentions", "none"] as NotificationLevel[]).map((l) => (
          <button
            key={l}
            onClick={() => setDefaultNotificationLevel(l)}
            style={{
              flex: 1, padding: "10px 8px", borderRadius: 6, cursor: "pointer",
              border: `2px solid ${defaultNotificationLevel === l ? "var(--color-bc-accent)" : "var(--color-bc-surface-3)"}`,
              background: "var(--color-bc-surface-2)", color: "var(--color-bc-text)", fontWeight: 500, fontSize: 14,
            }}
          >
            {l === "all" ? "All Messages" : l === "mentions" ? "Mentions Only" : "None"}
          </button>
        ))}
      </div>

      <SectionHeader>Delivery</SectionHeader>
      <SettingRow label="OS Notifications" description="Show desktop notifications when the app is in background">
        <Toggle checked={osNotificationsEnabled} onChange={setOsNotificationsEnabled} />
      </SettingRow>
      <SettingRow label="Notification Sound" description="Play a sound for new notifications">
        <Toggle checked={soundEnabled} onChange={setSoundEnabled} />
      </SettingRow>

      <SectionHeader>Do Not Disturb</SectionHeader>
      <SettingRow label="Enable DND Schedule" description="Suppress notifications during scheduled hours">
        <Toggle checked={dndSchedule.enabled} onChange={(v) => setDndSchedule({ ...dndSchedule, enabled: v })} />
      </SettingRow>

      {dndSchedule.enabled && (
        <div style={{ display: "flex", gap: 16, marginTop: 12, padding: "12px", background: "var(--color-bc-surface-2)", borderRadius: 6 }}>
          <div style={{ flex: 1 }}>
            <label style={{ display: "block", fontSize: 12, color: "var(--color-bc-muted)", marginBottom: 6, textTransform: "uppercase", fontWeight: 600, letterSpacing: "0.04em" }}>
              Start
            </label>
            <select
              value={dndSchedule.startHour}
              onChange={(e) => setDndSchedule({ ...dndSchedule, startHour: Number(e.target.value) })}
              style={{ width: "100%", background: "var(--color-bc-surface-3)", color: "var(--color-bc-text)", border: "1px solid var(--color-bc-surface-3)", borderRadius: 4, padding: "6px 8px" }}
              aria-label="DND start hour"
            >
              {Array.from({ length: 24 }, (_, i) => (
                <option key={i} value={i}>{fmtHour(i)}</option>
              ))}
            </select>
          </div>
          <div style={{ flex: 1 }}>
            <label style={{ display: "block", fontSize: 12, color: "var(--color-bc-muted)", marginBottom: 6, textTransform: "uppercase", fontWeight: 600, letterSpacing: "0.04em" }}>
              End
            </label>
            <select
              value={dndSchedule.endHour}
              onChange={(e) => setDndSchedule({ ...dndSchedule, endHour: Number(e.target.value) })}
              style={{ width: "100%", background: "var(--color-bc-surface-3)", color: "var(--color-bc-text)", border: "1px solid var(--color-bc-surface-3)", borderRadius: 4, padding: "6px 8px" }}
              aria-label="DND end hour"
            >
              {Array.from({ length: 24 }, (_, i) => (
                <option key={i} value={i}>{fmtHour(i)}</option>
              ))}
            </select>
          </div>
        </div>
      )}

      <div style={{ marginTop: 16, padding: "10px 14px", borderRadius: 6, background: "rgba(88,101,242,0.08)", fontSize: 13, color: "var(--color-bc-muted)" }}>
        Per-community and per-channel overrides can be configured from the community settings menu.
      </div>
    </div>
  );
}

// ── Tab: Advanced ─────────────────────────────────────────────────────────────

function AdvancedTab() {
  const [config, setConfig] = useState<NodeConfigDto | null>(null);
  const [listenAddrs, setListenAddrs] = useState<string[]>([]);
  const [newAddr, setNewAddr] = useState("");
  const [logLevel, setLogLevel] = useState("info");
  const [maxConns, setMaxConns] = useState(50);
  const [nodeMode, setNodeMode] = useState<import("../lib/rpc-types").NodeMode>("peer");
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState("");

  useEffect(() => {
    rpcClient.nodeGetConfig().then((c) => {
      setConfig(c);
      setListenAddrs([...c.listen_addrs]);
      setLogLevel(c.log_level);
      setMaxConns(c.max_connections);
      setNodeMode(c.node_mode);
    });
  }, []);

  async function save() {
    setSaving(true);
    try {
      await rpcClient.nodeSetConfig({ listen_addrs: listenAddrs, log_level: logLevel, max_connections: maxConns, node_mode: nodeMode });
      setSaveMsg("Saved!"); setTimeout(() => setSaveMsg(""), 2000);
    } catch {
      setSaveMsg("Failed to save");
    } finally {
      setSaving(false);
    }
  }

  function exportLogs() {
    const content = getLogText();
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `bitcord-logs-${Date.now()}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  }

  function addAddr() {
    const trimmed = newAddr.trim();
    if (!trimmed || listenAddrs.includes(trimmed)) return;
    setListenAddrs([...listenAddrs, trimmed]);
    setNewAddr("");
  }

  const { resetToDefaults } = useSettingsStore();
  const [resetConfirm, setResetConfirm] = useState(false);

  return (
    <div>
      <SectionHeader>Network</SectionHeader>

      <SettingRow label="Embedded Server" description="Run a local QUIC server to accept incoming peer connections. Disable to act as a client only (requires a seed node; reduces resource usage and security surface). Takes effect on next launch.">
        <Toggle checked={nodeMode === "peer"} onChange={(checked) => setNodeMode(checked ? "peer" : "gossip_client")} />
      </SettingRow>

      <SettingRow label="Max Connections" description="Maximum simultaneous peer connections">
        <input
          type="number" min={5} max={500} value={maxConns}
          onChange={(e) => setMaxConns(Number(e.target.value))}
          style={{ width: 72, background: "var(--color-bc-surface-2)", color: "var(--color-bc-text)", border: "1px solid var(--color-bc-surface-3)", borderRadius: 4, padding: "4px 8px", fontSize: 14 }}
          aria-label="Max connections"
        />
      </SettingRow>

      {config && (
        <div style={{ padding: "12px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
          <div style={{ color: "var(--color-bc-text)", fontWeight: 500, marginBottom: 8 }}>Listen Addresses</div>
          <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
            <div style={{ flex: 1 }}>
              <TextInput value={newAddr} onChange={setNewAddr} placeholder="e.g. 0.0.0.0:7332" />
            </div>
            <Btn onClick={addAddr} variant="ghost"><Plus size={14} /></Btn>
          </div>
          {listenAddrs.length === 0 && <div style={{ fontSize: 13, color: "var(--color-bc-muted)" }}>No custom listen addresses (using defaults)</div>}
          {listenAddrs.map((addr) => (
            <div key={addr} style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4, background: "var(--color-bc-surface-2)", borderRadius: 4, padding: "4px 10px" }}>
              <span style={{ flex: 1, fontFamily: "monospace", fontSize: 12, wordBreak: "break-all" }}>{addr}</span>
              <button onClick={() => setListenAddrs(listenAddrs.filter((a) => a !== addr))} style={{ background: "none", border: "none", cursor: "pointer", color: "var(--color-bc-muted)" }} aria-label="Remove address">
                <Trash2 size={13} />
              </button>
            </div>
          ))}
        </div>
      )}

      <SectionHeader>Logging</SectionHeader>
      <SettingRow label="Log Level">
        <Select value={logLevel} onChange={setLogLevel} options={[
          { value: "error", label: "Error" },
          { value: "warn", label: "Warn" },
          { value: "info", label: "Info" },
          { value: "debug", label: "Debug" },
          { value: "trace", label: "Trace" },
        ]} />
      </SettingRow>

      <div style={{ marginTop: 16, display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <Btn onClick={save} disabled={saving || !config}>{saving ? "Saving…" : "Save Advanced Settings"}</Btn>
        <Btn variant="ghost" onClick={exportLogs}>Export Logs</Btn>
        {saveMsg && <span style={{ fontSize: 13, color: saveMsg === "Saved!" ? "var(--color-bc-success)" : "var(--color-bc-danger)" }}>{saveMsg}</span>}
      </div>

      <SectionHeader>Reset</SectionHeader>
      {!resetConfirm ? (
        <Btn variant="danger" onClick={() => setResetConfirm(true)}>Reset Appearance to Defaults</Btn>
      ) : (
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span style={{ fontSize: 13, color: "var(--color-bc-muted)" }}>Are you sure?</span>
          <Btn variant="danger" onClick={() => { resetToDefaults(); setResetConfirm(false); }}>Yes, Reset</Btn>
          <Btn variant="ghost" onClick={() => setResetConfirm(false)}>Cancel</Btn>
        </div>
      )}
    </div>
  );
}

// ── Tab: About ────────────────────────────────────────────────────────────────

function AboutTab() {
  const identity = useIdentityStore((s) => s.identity);
  const { copied, copy } = useCopyText();
  const BUILD_COMMIT = import.meta.env.VITE_BUILD_COMMIT ?? "development";
  const APP_VERSION = import.meta.env.VITE_APP_VERSION ?? "0.2.0";

  return (
    <div>
      <div style={{ display: "flex", alignItems: "center", gap: 20, marginBottom: 32 }}>
        <div style={{
          width: 72, height: 72, borderRadius: 16,
          background: "var(--color-bc-accent)", display: "flex", alignItems: "center", justifyContent: "center",
          fontSize: 32, fontWeight: 800, color: "white", flexShrink: 0,
        }}>
          B
        </div>
        <div>
          <div style={{ fontSize: 24, fontWeight: 700, color: "var(--color-bc-text)" }}>BitCord</div>
          <div style={{ fontSize: 14, color: "var(--color-bc-muted)", marginTop: 4 }}>
            Decentralized, encrypted P2P messaging
          </div>
        </div>
      </div>

      <SectionHeader>Version Info</SectionHeader>

      {[
        { label: "App Version", value: `v${APP_VERSION}` },
        { label: "Build Commit", value: BUILD_COMMIT },
      ].map(({ label, value }) => (
        <div key={label} style={{ display: "flex", justifyContent: "space-between", padding: "8px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
          <span style={{ color: "var(--color-bc-muted)" }}>{label}</span>
          <span style={{ fontFamily: "monospace", fontSize: 13, color: "var(--color-bc-text)" }}>{value}</span>
        </div>
      ))}

      <SectionHeader>Identity</SectionHeader>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "8px 0", borderBottom: "1px solid var(--color-bc-surface-3)" }}>
        <span style={{ color: "var(--color-bc-muted)" }}>Peer ID</span>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <span style={{ fontFamily: "monospace", fontSize: 12, color: "var(--color-bc-text)", maxWidth: 320, wordBreak: "break-all", textAlign: "right" }}>
            {identity?.peer_id ?? "—"}
          </span>
          <button onClick={() => identity?.peer_id && copy(identity.peer_id)} aria-label="Copy Peer ID"
            style={{ background: "none", border: "none", cursor: "pointer", color: "var(--color-bc-muted)" }}>
            {copied ? <Check size={14} color="var(--color-bc-success)" /> : <Copy size={14} />}
          </button>
        </div>
      </div>

      <SectionHeader>Links</SectionHeader>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <a
          href="https://github.com/bitcord-net/BitCord"
          target="_blank"
          rel="noopener noreferrer"
          style={{ color: "var(--color-bc-accent)", fontSize: 14, textDecoration: "none" }}
        >
          Source Code on GitHub →
        </a>
      </div>

      <div style={{ marginTop: 32, fontSize: 12, color: "var(--color-bc-muted)" }}>
        BitCord is open-source software. All messages are end-to-end encrypted.
      </div>
    </div>
  );
}

// ── Settings Page ─────────────────────────────────────────────────────────────

type TabId = "account" | "node" | "appearance" | "notifications" | "advanced" | "about";

const TABS: { id: TabId; label: string; icon: React.ReactNode }[] = [
  { id: "account", label: "Account", icon: <User size={16} /> },
  { id: "node", label: "Node", icon: <Server size={16} /> },
  { id: "appearance", label: "Appearance", icon: <Palette size={16} /> },
  { id: "notifications", label: "Notifications", icon: <Bell size={16} /> },
  { id: "advanced", label: "Advanced", icon: <Wrench size={16} /> },
  { id: "about", label: "About", icon: <Info size={16} /> },
];

export function SettingsPage() {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useState<TabId>("account");

  // Close on Escape
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") navigate(-1);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  function renderTab() {
    switch (activeTab) {
      case "account": return <AccountTab />;
      case "node": return <NodeTab />;
      case "appearance": return <AppearanceTab />;
      case "notifications": return <NotificationsTab />;
      case "advanced": return <AdvancedTab />;
      case "about": return <AboutTab />;
    }
  }

  return (
    <div style={{
      position: "fixed", inset: 0, background: "rgba(0,0,0,0.7)", zIndex: 1000,
      display: "flex", alignItems: "stretch", justifyContent: "center",
    }}
      onClick={(e) => { if (e.target === e.currentTarget) navigate(-1); }}
      role="dialog" aria-modal="true" aria-label="Settings"
    >
      <div style={{
        display: "flex", flex: 1, maxWidth: 1100, background: "var(--color-bc-base)",
      }}>
        {/* Sidebar */}
        <nav style={{
          width: 220, background: "var(--color-bc-surface-1)", padding: "60px 8px 16px",
          flexShrink: 0, overflowY: "auto",
        }}
          aria-label="Settings navigation"
        >
          <div style={{ fontSize: 11, fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--color-bc-muted)", padding: "0 8px", marginBottom: 4 }}>
            User Settings
          </div>
          {TABS.map((tab) => (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
              aria-current={activeTab === tab.id ? "page" : undefined}
              style={{
                display: "flex", alignItems: "center", gap: 8, width: "100%",
                padding: "8px 10px", borderRadius: 4, border: "none", cursor: "pointer",
                background: activeTab === tab.id ? "var(--color-bc-surface-hover)" : "transparent",
                color: activeTab === tab.id ? "var(--color-bc-text)" : "var(--color-bc-muted)",
                fontSize: 14, fontWeight: activeTab === tab.id ? 500 : 400,
                textAlign: "left", transition: "background 0.1s",
              }}
            >
              {tab.icon}
              {tab.label}
            </button>
          ))}
        </nav>

        {/* Content */}
        <main style={{ flex: 1, padding: "60px 40px 40px", overflowY: "auto", minWidth: 0 }}>
          <div style={{ maxWidth: 660 }}>
            <h2 style={{ margin: "0 0 24px", fontSize: 20, fontWeight: 700, color: "var(--color-bc-text)" }}>
              {TABS.find((t) => t.id === activeTab)?.label}
            </h2>
            {renderTab()}
          </div>
        </main>

        {/* Close button */}
        <div style={{ position: "absolute", top: 16, right: 16 }}>
          <button
            onClick={() => navigate(-1)}
            aria-label="Close settings"
            style={{
              background: "var(--color-bc-surface-3)", border: "none", borderRadius: "50%",
              width: 36, height: 36, display: "flex", alignItems: "center", justifyContent: "center",
              cursor: "pointer", color: "var(--color-bc-text)",
            }}
          >
            <X size={18} />
          </button>
        </div>
      </div>
    </div>
  );
}
