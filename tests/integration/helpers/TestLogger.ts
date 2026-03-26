/**
 * TestLogger — builds a structured JSON test report suitable for LLM consumption.
 *
 * Each test scenario creates one TestLogger, records steps as they pass/fail,
 * and calls `writeReport()` at the end to persist the report to test-results/.
 */

import { mkdirSync, writeFileSync } from "fs";
import { join } from "path";

export type StepStatus = "pass" | "fail" | "skip";

export interface StepRecord {
  step: string;
  status: StepStatus;
  duration_ms: number;
  meta?: Record<string, unknown>;
  error?: string;
}

export interface NodeLogLine {
  timestamp: string;
  level: string;
  message: string;
  source: string;
}

export interface TestReport {
  suite: string;
  started_at: string;
  finished_at: string;
  total: number;
  passed: number;
  failed: number;
  steps: StepRecord[];
  node_logs: NodeLogLine[];
}

export class TestLogger {
  private suiteName: string;
  private startedAt: string;
  private steps: StepRecord[] = [];
  private nodeLogs: NodeLogLine[] = [];

  constructor(suiteName: string) {
    this.suiteName = suiteName;
    this.startedAt = new Date().toISOString();
  }

  /** Record the timing and result of a single test step. */
  async step<T>(
    name: string,
    fn: () => Promise<T>,
    meta?: Record<string, unknown>
  ): Promise<T> {
    const t0 = Date.now();
    try {
      const result = await fn();
      this.steps.push({
        step: name,
        status: "pass",
        duration_ms: Date.now() - t0,
        meta,
      });
      return result;
    } catch (err) {
      this.steps.push({
        step: name,
        status: "fail",
        duration_ms: Date.now() - t0,
        meta,
        error: err instanceof Error ? err.message : String(err),
      });
      throw err;
    }
  }

  /** Add node log lines collected from a NodeProcess instance. */
  addNodeLogs(logs: NodeLogLine[]): void {
    this.nodeLogs.push(...logs);
  }

  /** Write the JSON report to `tests/integration/test-results/<suite>.json`. */
  writeReport(): void {
    const passed = this.steps.filter((s) => s.status === "pass").length;
    const failed = this.steps.filter((s) => s.status === "fail").length;

    const report: TestReport = {
      suite: this.suiteName,
      started_at: this.startedAt,
      finished_at: new Date().toISOString(),
      total: this.steps.length,
      passed,
      failed,
      steps: this.steps,
      node_logs: this.nodeLogs,
    };

    const outDir = join(import.meta.dirname, "..", "test-results");
    mkdirSync(outDir, { recursive: true });

    const outPath = join(outDir, `${this.suiteName}.json`);
    writeFileSync(outPath, JSON.stringify(report, null, 2), "utf8");
    console.log(`[TestLogger] report written → ${outPath}`);
  }
}

/** Sleep for `ms` milliseconds. */
export const sleep = (ms: number): Promise<void> =>
  new Promise((r) => setTimeout(r, ms));

/** Poll `fn` every `intervalMs` until it returns truthy or `timeoutMs` is reached. */
export async function pollUntil<T>(
  fn: () => Promise<T | null | undefined>,
  timeoutMs: number,
  intervalMs = 300
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await fn();
    if (result != null) return result;
    await sleep(intervalMs);
  }
  throw new Error(`pollUntil: timed out after ${timeoutMs} ms`);
}
