import { createRoot } from "react-dom/client";
import { InstructionPanel } from "./InstructionPanel";

// The panel webview's root: a single React tree that renders the
// instruction panel. The panel webview is a separate Tauri Webview
// stacked above the content webview in the same window; it loads
// this page from our local Vite-built `panel.html` (asset protocol,
// not a remote URL), so Tauri IPC works without any capability
// remote-URL gymnastics.

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(<InstructionPanel />);
}
