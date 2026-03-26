import { Hash, Search, Volume2, Megaphone } from "lucide-react";
import type { ChannelInfo } from "../lib/rpc-types";

interface ChannelHeaderProps {
  channel: ChannelInfo;
  memberCount: number;
}

export function ChannelHeader({ channel, memberCount }: ChannelHeaderProps) {
  const icon =
    channel.kind === "voice" ? (
      <Volume2 size={18} aria-hidden="true" />
    ) : channel.kind === "announcement" ? (
      <Megaphone size={18} aria-hidden="true" />
    ) : (
      <Hash size={18} aria-hidden="true" />
    );

  return (
    <header
      style={{
        height: "48px",
        display: "flex",
        alignItems: "center",
        padding: "0 1rem",
        gap: "0.5rem",
        borderBottom: "1px solid rgba(255,255,255,0.06)",
        background: "var(--color-bc-surface-3)",
        flexShrink: 0,
      }}
    >
      <span style={{ color: "var(--color-bc-muted)", flexShrink: 0 }}>
        {icon}
      </span>
      <h1
        style={{
          margin: 0,
          fontSize: "1rem",
          fontWeight: 700,
          color: "var(--color-bc-text)",
          flex: 1,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {channel.name}
      </h1>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: "0.75rem",
          color: "var(--color-bc-muted)",
          fontSize: "0.875rem",
          flexShrink: 0,
        }}
      >
        <span aria-label={`${memberCount} members`}>{memberCount} members</span>
        <button
          title="Search (not implemented)"
          aria-label="Search messages"
          disabled
          style={{
            border: "none",
            background: "transparent",
            color: "var(--color-bc-muted)",
            cursor: "not-allowed",
            padding: "2px",
            display: "flex",
            borderRadius: "3px",
            opacity: 0.4,
          }}
        >
          <Search size={18} aria-hidden="true" />
        </button>
      </div>
    </header>
  );
}
