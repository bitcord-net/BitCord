/**
 * Run `tauri build` from the bitcord-tauri crate directory.
 *
 * The Tauri CLI scans the CWD for tauri.conf.json; since our Tauri crate lives
 * at crates/bitcord-tauri/ (not app/src-tauri/), we must cd there before
 * invoking the CLI.
 */
import { execFileSync } from "child_process";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const appDir    = resolve(__dirname, "..");
const tauriDir  = resolve(__dirname, "../../crates/bitcord-tauri");
const tauriBin  = resolve(__dirname, "../node_modules/.bin/tauri");

// Build the frontend first (beforeBuildCommand removed from tauri.conf.json to
// avoid path-resolution issues with relative --prefix on Windows).
console.log("[tauri-build] Building frontend...");
execFileSync("npm", ["run", "build"], { cwd: appDir, stdio: "inherit", shell: true });

// Then invoke tauri build from the crate directory where tauri.conf.json lives.
console.log("[tauri-build] Building Tauri app...");
execFileSync(tauriBin, ["build"], { cwd: tauriDir, stdio: "inherit", shell: true });
