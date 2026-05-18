import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  // The viewer iframe loads at
  // `plugin://localhost/psychological-operations/index.html`, so all
  // generated asset URLs must be document-relative. Without this the
  // emitted `<script src="/assets/…">` 404s under the custom scheme.
  base: "./",
  build: {
    // `dist/` contents are zipped verbatim into the release asset, so
    // `index.html` lands at the zip root — exactly what the host
    // expects under `<plugins_dir>/<repository>/viewer/`.
    outDir: "dist",
    emptyOutDir: true,
  },
});
