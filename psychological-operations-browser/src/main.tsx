import { createRoot } from "react-dom/client";
import App from "./App";
import { installSpaUrlReporter } from "./spa-url";

// This bundle is injected into x.com (and future psyop pages) via
// `WebviewWindowBuilder::initialization_script` on the Rust side,
// which maps to WebView2's `AddScriptToExecuteOnDocumentCreated`.
// It runs in the page's JS context before any page script runs.

// Host div sits at the top of <html>, full-viewport, pointer-events
// off so empty regions click through to the underlying x.com page.
// Individual overlay components opt in to `pointer-events: auto`.
const host = document.createElement("div");
host.id = "psyops-overlay";
host.style.cssText =
  "position:fixed;inset:0;pointer-events:none;z-index:2147483647;";
document.documentElement.appendChild(host);

// Closed Shadow DOM so x.com's CSS can't reach into the overlay
// and ours can't leak out.
const shadow = host.attachShadow({ mode: "closed" });
const mount = document.createElement("div");
shadow.appendChild(mount);

createRoot(mount).render(<App />);
installSpaUrlReporter();
