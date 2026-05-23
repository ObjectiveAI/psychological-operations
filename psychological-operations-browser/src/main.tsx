import { installConsoleCapture, drainConsole } from "./console-capture";

// Install console + exception capture as the FIRST thing the bundle
// does. The bundle is injected via `initialization_script` and runs
// before any page script, so anything we install here catches
// everything the page itself logs or throws.
installConsoleCapture();

import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event } from "@tauri-apps/api/event";
import App from "./App";
import { installSpaUrlReporter } from "./spa-url";

// This bundle is injected into every page the X-App webview loads
// (initially https://console.x.ai/, plus anywhere the user
// navigates to) via `WebviewWindowBuilder::initialization_script`,
// which maps to WebView2's `AddScriptToExecuteOnDocumentCreated` on
// Windows. It runs in the page's JS context *before any page
// script*, which means `document.documentElement` and
// `document.body` are both null at the moment this code runs —
// any DOM mount has to be deferred until at least `interactive`
// readyState.

type Request =
  | { type: "x_app" }
  | { type: "html" }
  | { type: "console" }
  | { type: "eval"; code: string };

type Mode = { type: "x_app" } | null;

let urlReporterUninstall: (() => void) | null = null;

function stopUrlReporter() {
  urlReporterUninstall?.();
  urlReporterUninstall = null;
}

async function respondOk(response: unknown) {
  await invoke("stdio_respond", {
    result: { status: "ok", response },
  }).catch(() => {});
}

async function respondErr(error: string) {
  await invoke("stdio_respond", {
    result: { status: "err", error },
  }).catch(() => {});
}

async function handleRequest(event: Event<Request>) {
  const req = event.payload;
  switch (req.type) {
    case "x_app": {
      // Ack FIRST — the user wants the ack on the wire before any URL
      // emission. Mode resets are the highest-priority signal.
      await respondOk({ type: "ack" });

      // Halt prior per-mode state. After the navigation below the
      // overlay will re-mount, query current_mode, and reinstall.
      stopUrlReporter();

      // Navigate (or reload if already on the right origin so the
      // overlay still re-mounts on the fresh page).
      const target = "https://console.x.ai/";
      if (location.href === target || location.href.startsWith(target)) {
        location.reload();
      } else {
        location.assign(target);
      }
      break;
    }
    case "html": {
      const html = document.documentElement.outerHTML;
      await respondOk({ type: "html", html });
      break;
    }
    case "console": {
      const entries = drainConsole();
      await respondOk({ type: "console", entries });
      break;
    }
    case "eval": {
      try {
        // Indirect-eval so the code runs in the global scope, not
        // a closure that would expose our overlay's locals to it.
        const result = (0, eval)(req.code);
        // Resolve thenables so `await fetch(...).then(r => r.json())`
        // style code works.
        const resolved = await Promise.resolve(result);
        // Round-trip through JSON to strip non-serializable bits
        // (functions, DOM nodes, etc. become null / get dropped).
        const safe = JSON.parse(JSON.stringify(resolved ?? null));
        await respondOk({ type: "eval", result: safe });
      } catch (e) {
        await respondErr((e as Error)?.stack ?? String(e));
      }
      break;
    }
  }
}

// ---------- Shadow-DOM overlay mount (deferred until DOM exists) ----------
function mountOverlay() {
  const root = document.body ?? document.documentElement;
  if (!root) return; // shouldn't happen past `interactive` readyState
  const host = document.createElement("div");
  host.id = "psyops-overlay";
  host.style.cssText =
    "position:fixed;inset:0;pointer-events:none;z-index:2147483647;";
  root.appendChild(host);

  const shadow = host.attachShadow({ mode: "closed" });
  const mount = document.createElement("div");
  shadow.appendChild(mount);
  createRoot(mount).render(<App />);
}

function whenDomReady(fn: () => void) {
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", fn, { once: true });
  } else {
    fn();
  }
}

// ---------- IPC handshake + mode resume ----------
//
// Mirror objectiveai-viewer/src/App.tsx:336 — register the listener
// FIRST, then signal Rust. The OS pipe holds anything the host
// wrote during startup; the Rust stdin reader blocks on this
// signal before reading. IPC works even before the DOM is ready,
// so we don't have to wait for DOMContentLoaded for this part.
(async () => {
  try {
    await listen<Request>("psyops:request", handleRequest);
    await invoke("frontend_ready");
    const mode = await invoke<Mode>("current_mode");
    if (mode?.type === "x_app") {
      // Re-mounted into an active session (e.g. after the XApp
      // handler's `location.assign`). Pick up where the previous
      // overlay left off and start URL reporting. The initial URL
      // is emitted by `installSpaUrlReporter` itself.
      urlReporterUninstall = installSpaUrlReporter();
    }
  } catch {
    // Best-effort — keep the overlay alive even if IPC failed.
  }
})();

whenDomReady(mountOverlay);
