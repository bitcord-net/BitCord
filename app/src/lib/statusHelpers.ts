import type { UserStatus } from "./rpc-types";

export function statusColor(status: string): string {
  switch (status as UserStatus) {
    case "online": return "var(--color-bc-success)";
    case "idle": return "var(--color-bc-warning)";
    case "do_not_disturb": return "var(--color-bc-danger)";
    default: return "var(--color-bc-muted)";
  }
}

export function statusLabel(status: string): string {
  switch (status as UserStatus) {
    case "online": return "Online";
    case "idle": return "Idle";
    case "do_not_disturb": return "Do Not Disturb";
    case "invisible": return "Invisible";
    default: return "Offline";
  }
}
