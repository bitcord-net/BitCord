import { MessageSquare } from "lucide-react";

export function DMListPlaceholder() {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        background: "var(--color-bc-surface-3)",
        color: "var(--color-bc-muted)",
        gap: "0.75rem",
      }}
    >
      <MessageSquare size={48} aria-hidden="true" style={{ opacity: 0.3 }} />
      <div style={{ textAlign: "center" }}>
        <p style={{ margin: "0 0 0.25rem", fontWeight: 600, color: "var(--color-bc-text)" }}>
          Your Messages
        </p>
        <p style={{ margin: 0, fontSize: "0.875rem" }}>
          Select a conversation or start a new one.
        </p>
      </div>
    </div>
  );
}
