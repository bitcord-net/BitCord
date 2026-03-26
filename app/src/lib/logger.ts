/**
 * In-memory log buffer.
 *
 * Call `initLogger()` once at app startup to wrap the global `console`
 * methods. All subsequent console output is captured in a fixed-size ring
 * buffer in addition to being forwarded to the original console.
 *
 * Call `getLogText()` to serialize the buffer for file export.
 */

const MAX_ENTRIES = 2000;

type LogLevel = "debug" | "info" | "warn" | "error";

interface LogEntry {
  ts: string;       // ISO timestamp
  level: LogLevel;
  msg: string;
}

const buffer: LogEntry[] = [];
let initialized = false;

function push(level: LogLevel, args: unknown[]) {
  const msg = args
    .map((a) =>
      typeof a === "string"
        ? a
        : a instanceof Error
        ? `${a.name}: ${a.message}`
        : JSON.stringify(a)
    )
    .join(" ");

  if (buffer.length >= MAX_ENTRIES) {
    buffer.shift();
  }
  buffer.push({ ts: new Date().toISOString(), level, msg });
}

/** Install console wrappers. Safe to call multiple times. */
export function initLogger() {
  if (initialized) return;
  initialized = true;

  const orig = {
    debug: console.debug.bind(console),
    log: console.log.bind(console),
    info: console.info.bind(console),
    warn: console.warn.bind(console),
    error: console.error.bind(console),
  };

  console.debug = (...args: unknown[]) => { push("debug", args); orig.debug(...args); };
  console.log   = (...args: unknown[]) => { push("info",  args); orig.log(...args); };
  console.info  = (...args: unknown[]) => { push("info",  args); orig.info(...args); };
  console.warn  = (...args: unknown[]) => { push("warn",  args); orig.warn(...args); };
  console.error = (...args: unknown[]) => { push("error", args); orig.error(...args); };

  // Capture unhandled promise rejections
  window.addEventListener("unhandledrejection", (e) => {
    push("error", [`UnhandledRejection: ${String(e.reason)}`]);
  });
}

/** Return a plain-text representation of the log buffer, newest last. */
export function getLogText(): string {
  if (buffer.length === 0) {
    return "(no log entries captured yet)";
  }
  const header = [
    `BitCord log export`,
    `Exported: ${new Date().toISOString()}`,
    `Entries: ${buffer.length} (max ${MAX_ENTRIES})`,
    "─".repeat(72),
    "",
  ].join("\n");

  const lines = buffer.map(
    ({ ts, level, msg }) => `${ts} [${level.toUpperCase().padEnd(5)}] ${msg}`
  );

  return header + lines.join("\n");
}
