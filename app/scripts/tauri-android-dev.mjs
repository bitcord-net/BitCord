/**
 * Run `tauri android dev` from the bitcord-tauri crate directory.
 * 
 * This script detects the local IP address to set TAURI_DEV_HOST,
 * ensuring the Android emulator can connect to the Vite dev server.
 */
import { spawn } from "child_process";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";
import { networkInterfaces } from "os";

const __dirname = dirname(fileURLToPath(import.meta.url));
const appDir    = resolve(__dirname, "..");
const tauriDir  = resolve(__dirname, "../../crates/bitcord-tauri");
const tauriBin  = resolve(__dirname, "../node_modules/.bin/tauri.cmd");

// Detect local IPv4 address
function getLocalIp() {
  const nets = networkInterfaces();
  for (const name of Object.keys(nets)) {
    for (const net of nets[name]) {
      // Skip over non-IPv4 and internal (i.e. 127.0.0.1) addresses
      if (net.family === 'IPv4' && !net.internal) {
        return net.address;
      }
    }
  }
  return '0.0.0.0';
}

const localIp = getLocalIp();
console.log(`[tauri-android-dev] Detected local IP: ${localIp}`);

// Start Vite in the background
const vite = spawn("npm", ["run", "dev"], {
  cwd: appDir,
  stdio: "inherit",
  shell: true,
  env: { ...process.env, VITE_HOST: "0.0.0.0" }
});

vite.on("error", (err) => console.error("[tauri-android-dev] Vite failed to start:", err));

// Wait for Vite to be ready, then launch Tauri
// We use TAURI_DEV_HOST to tell Tauri where to find the dev server
process.chdir(tauriDir);
const tauri = spawn(tauriBin, ["android", "dev"], {
  stdio: "inherit",
  shell: true,
  env: { 
    ...process.env, 
    TAURI_DEV_HOST: localIp 
  }
});

tauri.on("exit", (code) => {
  vite.kill();
  process.exit(code || 0);
});

process.on("SIGINT", () => {
  vite.kill();
  tauri.kill();
  process.exit(0);
});
