/**
 * NodeProcess — spawns and manages a `bitcord-node` child process for integration tests.
 *
 * Usage:
 *   const node = new NodeProcess({ apiPort: 7401, p2pPort: 7402, label: "node-a" });
 *   await node.start();
 *   // ... run tests ...
 *   await node.stop();
 */

import { spawn, ChildProcess } from "child_process";
import { mkdirSync, writeFileSync, rmSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import WebSocket from "ws";

export interface NodeProcessOptions {
  /** Label for this node (used in log output and temp dir names). */
  label: string;
  /** Port for the JSON-RPC API. */
  apiPort: number;
  /** Additional seed nodes to add to config (optional). */
  seedNodes?: string[];
  /** Log level (default: "info"). */
  logLevel?: string;
}

interface LogLine {
  timestamp: string;
  level: string;
  message: string;
  source: string;
}

export class NodeProcess {
  readonly label: string;
  readonly apiPort: number;
  private opts: NodeProcessOptions;
  private proc: ChildProcess | null = null;
  private dataDir: string;
  private configPath: string;
  readonly logBuffer: LogLine[] = [];

  constructor(opts: NodeProcessOptions) {
    this.opts = opts;
    this.label = opts.label;
    this.apiPort = opts.apiPort;
    this.dataDir = join(tmpdir(), `bitcord-test-${opts.label}-${process.pid}`);
    this.configPath = join(this.dataDir, "config.toml");
  }

  get apiUrl(): string {
    if (process.env["BITCORD_DOCKER_MODE"]) {
      return this._dockerUrl();
    }
    return `ws://127.0.0.1:${this.apiPort}`;
  }

  /** Start the node and wait until the API port is reachable (up to 15 s). */
  async start(): Promise<void> {
    if (process.env["BITCORD_DOCKER_MODE"]) {
      // In Docker mode the node is already running as a separate container.
      // Skip spawning and just wait for the API to become reachable.
      await this._waitForApi(30_000);
      return;
    }

    this._writeConfig();

    const binPath =
      process.env["BITCORD_NODE_BIN"] ??
      join(import.meta.dirname, "..", "..", "..", "target", "debug", "bitcord-node");

    this.proc = spawn(
      binPath,
      ["--config", this.configPath, "--api-port", String(this.apiPort)],
      {
        env: {
          ...process.env,
          BITCORD_PASSPHRASE: "",
          BITCORD_TEST_MODE: "1",
        },
        stdio: ["ignore", "pipe", "pipe"],
      }
    );

    // Collect JSON log lines for the test report.
    const collectLogs = (stream: NodeJS.ReadableStream): void => {
      let buf = "";
      stream.on("data", (chunk: Buffer) => {
        buf += chunk.toString();
        const lines = buf.split("\n");
        buf = lines.pop() ?? "";
        for (const line of lines) {
          if (!line.trim()) continue;
          try {
            const parsed = JSON.parse(line) as Record<string, unknown>;
            this.logBuffer.push({
              timestamp: String(parsed["timestamp"] ?? parsed["time"] ?? new Date().toISOString()),
              level: String(parsed["level"] ?? "INFO").toUpperCase(),
              message: String((parsed["fields"] as Record<string, unknown> | undefined)?.["message"] ?? parsed["msg"] ?? line),
              source: this.label,
            });
          } catch {
            // Non-JSON line (startup messages before JSON mode kicks in).
          }
        }
      });
    };

    if (this.proc.stdout) collectLogs(this.proc.stdout);
    if (this.proc.stderr) collectLogs(this.proc.stderr);

    this.proc.on("error", (err) => {
      console.error(`[${this.label}] process error:`, err);
    });

    await this._waitForApi(15_000);
  }

  /** Send SIGTERM and wait for the process to exit. */
  async stop(): Promise<void> {
    if (process.env["BITCORD_DOCKER_MODE"]) {
      // In Docker mode the node lifecycle is managed by docker compose — don't kill it.
      return;
    }
    if (!this.proc) return;
    this.proc.kill("SIGTERM");
    await new Promise<void>((resolve) => {
      this.proc!.once("exit", () => resolve());
      setTimeout(resolve, 5_000); // Force resolve after 5 s.
    });
    this.proc = null;
    this._cleanup();
  }

  // ── Internals ──────────────────────────────────────────────────────────────

  private _writeConfig(): void {
    mkdirSync(this.dataDir, { recursive: true });

    const seedNodesToml = (this.opts.seedNodes ?? [])
      .map((s) => `"${s}"`)
      .join(", ");
    const logLevel = this.opts.logLevel ?? "info";

    const toml = [
      `data_dir = "${this.dataDir.replace(/\\/g, "/")}/data"`,
      `identity_path = "${this.dataDir.replace(/\\/g, "/")}/identity.key"`,
      `seed_nodes = [${seedNodesToml}]`,
      `log_level = "${logLevel}"`,
      // quic_port = 0 lets the OS pick a free port, avoiding conflicts between
      // concurrently-running test nodes.
      `quic_port = 0`,
    ].join("\n");

    writeFileSync(this.configPath, toml, "utf8");
  }

  /**
   * Map this node's label to a pre-running Docker service URL.
   * Labels containing "-b" are treated as node-b; everything else as node-a.
   */
  private _dockerUrl(): string {
    if (this.label.includes("-b")) {
      return process.env["NODE_B_URL"] ?? "ws://node-b:7411";
    }
    return process.env["NODE_A_URL"] ?? "ws://node-a:7401";
  }

  private _waitForApi(timeoutMs: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const deadline = Date.now() + timeoutMs;

      const attempt = (): void => {
        const ws = new WebSocket(this.apiUrl);
        ws.once("open", () => {
          ws.close();
          resolve();
        });
        ws.once("error", () => {
          ws.close();
          if (Date.now() >= deadline) {
            reject(new Error(`[${this.label}] API did not become ready within ${timeoutMs} ms`));
          } else {
            setTimeout(attempt, 300);
          }
        });
      };

      attempt();
    });
  }

  private _cleanup(): void {
    try {
      rmSync(this.dataDir, { recursive: true, force: true });
    } catch {
      // Best-effort cleanup.
    }
  }
}
