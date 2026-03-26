import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [tailwindcss(), react()],
  clearScreen: false,
  server: {
    port: Number(process.env.VITE_PORT ?? 1420),
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || true,
    hmr: {
      protocol: 'ws',
      host: process.env.TAURI_DEV_HOST || 'localhost',
      port: Number(process.env.VITE_HMR_PORT ?? 1421),
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: ['es2021', 'chrome105', 'safari13'],
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
})
