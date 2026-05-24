import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// Build #1: the panel webview's HTML page (panel.html + ESM chunks).
// Built first; clears `dist/` to remove stale artifacts. The second
// build (vite.config.overlay.ts) runs after and appends overlay.js
// without wiping dist again (emptyOutDir: false there).
export default defineConfig({
  plugins: [react()],

  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    rollupOptions: {
      input: "panel.html",
    },
    outDir: "dist",
    emptyOutDir: true,
  },
});
