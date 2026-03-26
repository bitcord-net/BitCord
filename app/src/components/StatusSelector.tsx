import { useState, useRef, useEffect } from "react";
import { ChevronUp } from "lucide-react";
import type { UserStatus } from "../lib/rpc-types";
import { statusColor, statusLabel } from "../lib/statusHelpers";
import { useIdentityStore } from "../store/identity";

const STATUS_OPTIONS: UserStatus[] = ["online", "idle", "do_not_disturb", "invisible"];

export function StatusSelector() {
  const [open, setOpen] = useState(false);
  const [menuPos, setMenuPos] = useState<{ bottom: number; left: number } | null>(null);
  const { identity, setStatus } = useIdentityStore();
  const containerRef = useRef<HTMLDivElement>(null);
  const currentStatus = identity?.status ?? "offline";

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (!containerRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  const handleSelect = async (s: UserStatus) => {
    setOpen(false);
    await setStatus(s);
  };

  return (
    <div ref={containerRef} style={{ position: "relative" }}>
      <button
        onClick={() => {
          if (!open && containerRef.current) {
            const rect = containerRef.current.getBoundingClientRect();
            setMenuPos({ bottom: window.innerHeight - rect.top + 4, left: rect.left });
          }
          setOpen((v) => !v);
        }}
        title="Set status"
        aria-label="Set your status"
        aria-expanded={open}
        style={{
          background: "none",
          border: "none",
          cursor: "pointer",
          padding: "2px 4px",
          display: "flex",
          alignItems: "center",
          gap: "4px",
          borderRadius: "3px",
          color: "var(--color-bc-muted)",
          transition: "color 0.15s",
        }}
        onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.color = "var(--color-bc-text)")}
        onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.color = "var(--color-bc-muted)")}
      >
        <div
          style={{
            width: "8px",
            height: "8px",
            borderRadius: "50%",
            background: statusColor(currentStatus),
            flexShrink: 0,
          }}
        />
        <ChevronUp size={12} aria-hidden="true" />
      </button>

      {open && menuPos && (
        <div
          role="menu"
          aria-label="Status options"
          style={{
            position: "fixed",
            bottom: menuPos.bottom,
            left: menuPos.left,
            background: "var(--color-bc-surface-1)",
            border: "1px solid rgba(255,255,255,0.06)",
            borderRadius: "6px",
            boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
            padding: "0.25rem",
            zIndex: 400,
            minWidth: "180px",
          }}
        >
          {STATUS_OPTIONS.map((s) => (
            <button
              key={s}
              role="menuitem"
              onClick={() => void handleSelect(s)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.625rem",
                width: "100%",
                padding: "0.4375rem 0.625rem",
                background: currentStatus === s ? "var(--color-bc-surface-hover)" : "none",
                border: "none",
                cursor: "pointer",
                borderRadius: "4px",
                fontSize: "0.875rem",
                color: "var(--color-bc-text)",
                textAlign: "left",
              }}
              onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.background = "var(--color-bc-surface-hover)")}
              onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.background = currentStatus === s ? "var(--color-bc-surface-hover)" : "none")}
            >
              <div
                style={{
                  width: "10px",
                  height: "10px",
                  borderRadius: "50%",
                  background: statusColor(s),
                  flexShrink: 0,
                }}
              />
              {statusLabel(s)}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
