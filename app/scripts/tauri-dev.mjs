/**
 * Run `tauri dev` from the bitcord-tauri crate directory.
 *
 * The Tauri CLI scans the CWD for tauri.conf.json; since our Tauri crate lives
 * at crates/bitcord-tauri/ (not app/src-tauri/), we must cd there before
 * invoking the CLI. We also start the Vite dev server here so that Tauri's
 * window has something to connect to when it opens.
 */
import { execFileSync, spawn } from "child_process";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const appDir    = resolve(__dirname, "..");                        // app/
const tauriDir  = resolve(__dirname, "../../crates/bitcord-tauri");
const tauriBin  = resolve(__dirname, "../node_modules/.bin/tauri");

// Start Vite in the background (inherits stdio so output appears inline).
const vite = spawn("npm", ["run", "dev"], {
  cwd: appDir,
  stdio: "inherit",
  shell: true,
  detached: false,
});

vite.on("error", (err) => console.error("[tauri-dev] Vite failed to start:", err));

// Give Vite a moment to bind, then launch Tauri from the correct directory.
process.chdir(tauriDir);
try {
  execFileSync(tauriBin, ["dev"], { stdio: "inherit", shell: true });
} finally {
  vite.kill();
}
