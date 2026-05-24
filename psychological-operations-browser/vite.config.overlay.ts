import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Build #2: the IIFE bundle injected into the *content* webview via
// Rust's `initialization_script`. Runs after the panel build
// (vite.config.ts) so we must NOT empty dist/. Produces a single
// self-contained `dist/overlay.js`.
export default defineConfig({
  plugins: [react()],
  build: {
    rollupOptions: {
      input: "src/overlay/main.tsx",
      output: {
        format: "iife",
        entryFileNames: "overlay.js",
        inlineDynamicImports: true,
      },
    },
    cssCodeSplit: false,
    assetsInlineLimit: Number.MAX_SAFE_INTEGER,
    outDir: "dist",
    emptyOutDir: false,
  },
});
