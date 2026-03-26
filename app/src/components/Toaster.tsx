import { X } from "lucide-react";
import { useToastStore } from "../store/toast";
import type { Toast } from "../store/toast";

const KIND_COLORS: Record<Toast["kind"], { bg: string; border: string; icon: string }> = {
  info: {
    bg: "var(--color-bc-surface-2)",
    border: "rgba(255,255,255,0.1)",
    icon: "#5865f2",
  },
  warn: {
    bg: "rgba(250,166,26,0.12)",
    border: "rgba(250,166,26,0.35)",
    icon: "#faa61a",
  },
  error: {
    bg: "rgba(237,66,69,0.12)",
    border: "rgba(237,66,69,0.35)",
    icon: "#ed4245",
  },
  success: {
    bg: "rgba(67,181,129,0.12)",
    border: "rgba(67,181,129,0.35)",
    icon: "#43b581",
  },
};

export function Toaster() {
  const { toasts, dismiss } = useToastStore();
  if (toasts.length === 0) return null;

  return (
    <div
      aria-live="polite"
      aria-label="Notifications"
      style={{
        position: "fixed",
        bottom: "1.25rem",
        right: "1.25rem",
        display: "flex",
        flexDirection: "column",
        gap: "0.5rem",
        zIndex: 9999,
        pointerEvents: "none",
      }}
    >
      {toasts.map((t) => {
        const colors = KIND_COLORS[t.kind];
        return (
          <div
            key={t.id}
            role="status"
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.625rem",
              background: colors.bg,
              border: `1px solid ${colors.border}`,
              borderRadius: "8px",
              padding: "0.625rem 0.875rem",
              boxShadow: "0 4px 16px rgba(0,0,0,0.45)",
              maxWidth: "340px",
              pointerEvents: "all",
              animation: "toast-in 0.18s ease",
            }}
          >
            <span
              style={{
                width: "6px",
                height: "6px",
                borderRadius: "50%",
                background: colors.icon,
                flexShrink: 0,
              }}
            />
            <span
              style={{
                flex: 1,
                fontSize: "0.875rem",
                color: "var(--color-bc-text)",
                lineHeight: 1.4,
              }}
            >
              {t.message}
            </span>
            <button
              onClick={() => dismiss(t.id)}
              aria-label="Dismiss notification"
              style={{
                border: "none",
                background: "transparent",
                color: "var(--color-bc-muted)",
                cursor: "pointer",
                padding: "2px",
                display: "flex",
                borderRadius: "3px",
                flexShrink: 0,
              }}
            >
              <X size={14} />
            </button>
          </div>
        );
      })}
      <style>{`
        @keyframes toast-in {
          from { opacity: 0; transform: translateY(8px); }
          to   { opacity: 1; transform: translateY(0); }
        }
      `}</style>
    </div>
  );
}
