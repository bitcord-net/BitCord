import { statusColor, statusLabel } from "../lib/statusHelpers";

interface Props {
  status: string;
  size?: number;
  borderColor?: string;
}

export function PresenceIndicator({ status, size = 10, borderColor = "var(--color-bc-surface-2)" }: Props) {
  return (
    <div
      aria-hidden="true"
      title={statusLabel(status)}
      style={{
        width: `${size}px`,
        height: `${size}px`,
        borderRadius: "50%",
        background: statusColor(status),
        border: `2px solid ${borderColor}`,
        flexShrink: 0,
      }}
    />
  );
}
