import { installConsoleCapture, drainConsole } from "./console-capture";

// Install console + exception capture as the FIRST thing the bundle
// does. This bundle is injected into the CEF content browser via
// `Frame::execute_javascript` from `LoadHandler::on_load_start`,
// which fires before any page script runs. So anything we install
// here catches everything the page itself logs or throws.
//
// The instruction panel does NOT live in this bundle — it's a
// separate webview (Tauri/WebView2, loaded from our local panel.html)
// sitting above the CEF browser in the same window. This bundle only
// handles content-page-scoped concerns: stdio request dispatch, SPA
// URL reporting, console capture, and MutationObserver-driven overlay
// components (HandPointer, sign-in pointer, etc.).
installConsoleCapture();

import { invoke, registerPushHandler } from "./ipc";
import { installSpaUrlReporter } from "./spa-url";
import { installOnboardingHelpers } from "./onboarding-helpers";
import { installAppsTabHelper } from "./apps-tab-helper";
import { installAppsPageHelpers } from "./apps-page-helpers";
import { installAppPageHelpers } from "./app-page-helpers";
import { installAuthSettingsHelpers } from "./auth-settings-helpers";
import { installCreateAppDialogHelpers } from "./create-app-dialog-helpers";
import { installPostCreateDialogHelpers } from "./post-create-dialog-helpers";
import { installPsyopReadHelpers } from "./psyop-read-helpers";
// Side-effect: registers `window.__psyops_set_panel` so the first
// Rust push lands. Imported before any helper module so the setter
// is in place by the time anyone reads `getPanelState()`.
import type { PanelState } from "./panel-state";
import "./panel-state";

type Request =
  | { type: "html" }
  | { type: "console" }
  | { type: "eval"; code: string };

// Mode is locked at the browser binary's CLI flag and injected
// into the renderer as `window.__PSYOPS_MODE` by
// `cef::InjectOverlay::on_load_start` before this bundle runs.
type Mode =
  | { type: "x_app" }
  | { type: "psyop_read"; name: string }
  | { type: "psyop_authorize"; name: string }
  | null;

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

async function handleRequest(payload: unknown) {
  const req = payload as Request;
  switch (req.type) {
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
        const result = (0, eval)(req.code);
        const resolved = await Promise.resolve(result);
        const safe = JSON.parse(JSON.stringify(resolved ?? null));
        await respondOk({ type: "eval", result: safe });
      } catch (e) {
        await respondErr((e as Error)?.stack ?? String(e));
      }
      break;
    }
  }
}

// ---------- IPC handshake + helper install ----------
//
// Register the push handler FIRST, then invoke frontend_ready. The
// Rust stdin reader blocks on this signal before reading; the OS
// pipe buffers anything the host wrote during startup until then.
//
// The push handler is `window.__psyops.push` — Rust calls
// `frame.execute_javascript("window.__psyops.push(<json>)")` to
// deliver each request. Synchronous from JS's perspective; the ack
// goes back via `invoke("stdio_respond", ...)` per-request.
//
// Mode is read synchronously from the injected `__PSYOPS_MODE`
// global — no round-trip needed.
(async () => {
  console.log("[psyops-overlay] mount begin");
  try {
    registerPushHandler(handleRequest);
    console.log("[psyops-overlay] push handler registered, calling frontend_ready");
    await invoke("frontend_ready");
    console.log("[psyops-overlay] frontend_ready ok");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const mode = ((window as any).__PSYOPS_MODE ?? null) as Mode;
    console.log("[psyops-overlay] mode =", JSON.stringify(mode));
    // Seed the panel-state mirror BEFORE installing helpers so the
    // first tick of any pointer reads the actual state, not null.
    // Subsequent updates land via the Rust→JS push registered in
    // panel-state.ts.
    const initialPanel = await invoke<PanelState | null>("current_panel");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).__psyops_set_panel(initialPanel);
    console.log(
      "[psyops-overlay] initial panel =",
      JSON.stringify(initialPanel),
    );
    if (mode !== null) {
      // URL reporter is mode-agnostic — useful in every mode so
      // Rust's panel derivation has fresh `current_url` facts.
      installSpaUrlReporter();
      if (mode.type === "x_app") {
        installOnboardingHelpers();
        installAppsTabHelper();
        installAppsPageHelpers();
        installAppPageHelpers();
        installAuthSettingsHelpers();
        installCreateAppDialogHelpers();
        installPostCreateDialogHelpers();
      } else if (mode.type === "psyop_read") {
        installPsyopReadHelpers();
      }
      // psyop_authorize installs no helpers — Rust drives the
      // OAuth navigation on its own; X's consent page is the
      // affordance.
      console.log("[psyops-overlay] helpers installed for mode", mode.type);
    }
  } catch (e) {
    console.error("[psyops-overlay] mount failed:", (e as Error)?.message ?? String(e));
  }
})();
