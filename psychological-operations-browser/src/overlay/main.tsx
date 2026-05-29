import { installConsoleCapture, drainConsole } from "./console-capture";

// Install console + exception capture as the FIRST thing the bundle
// does. This bundle is injected into the X-App *content* CEF browser
// (which loads https://console.x.com/ + navigates to anywhere from
// there) via `Frame::execute_javascript` from
// `LoadHandler::on_load_start`, which fires before any page script
// runs. So anything we install here catches everything the page
// itself logs or throws.
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
import { installCreateAppDialogHelpers } from "./create-app-dialog-helpers";
import { installPostCreateDialogHelpers } from "./post-create-dialog-helpers";

type Request =
  | { type: "x_app" }
  | { type: "html" }
  | { type: "console" }
  | { type: "eval"; code: string };

type Mode = { type: "x_app" } | null;

let urlReporterUninstall: (() => void) | null = null;
let onboardingHelpersUninstall: (() => void) | null = null;
let appsTabHelperUninstall: (() => void) | null = null;
let appsPageHelpersUninstall: (() => void) | null = null;
let createAppDialogHelpersUninstall: (() => void) | null = null;
let postCreateDialogHelpersUninstall: (() => void) | null = null;

function stopUrlReporter() {
  urlReporterUninstall?.();
  urlReporterUninstall = null;
}

function stopOnboardingHelpers() {
  onboardingHelpersUninstall?.();
  onboardingHelpersUninstall = null;
}

function stopAppsTabHelper() {
  appsTabHelperUninstall?.();
  appsTabHelperUninstall = null;
}

function stopAppsPageHelpers() {
  appsPageHelpersUninstall?.();
  appsPageHelpersUninstall = null;
}

function stopCreateAppDialogHelpers() {
  createAppDialogHelpersUninstall?.();
  createAppDialogHelpersUninstall = null;
}

function stopPostCreateDialogHelpers() {
  postCreateDialogHelpersUninstall?.();
  postCreateDialogHelpersUninstall = null;
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

async function handleRequest(payload: unknown) {
  const req = payload as Request;
  switch (req.type) {
    case "x_app": {
      // Ack first so the host process gets the ack before any URL
      // or other side-effects.
      await respondOk({ type: "ack" });

      // Halt prior per-mode state. After the navigation below the
      // overlay will re-mount, query current_mode, and reinstall.
      stopUrlReporter();
      stopOnboardingHelpers();
      stopAppsTabHelper();
      stopAppsPageHelpers();
      stopCreateAppDialogHelpers();
      stopPostCreateDialogHelpers();

      // Navigate (or reload if already on the right origin so the
      // overlay still re-mounts on the fresh page).
      const target = "https://console.x.com/";
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

// ---------- IPC handshake + URL reporter on mode resume ----------
//
// Register the push handler FIRST, then invoke frontend_ready. The
// Rust stdin reader blocks on this signal before reading; the OS
// pipe buffers anything the host wrote during startup until then.
//
// The push handler is `window.__psyops.push` — Rust calls
// `frame.execute_javascript("window.__psyops.push(<json>)")` to
// deliver each request. Synchronous from JS's perspective; the ack
// goes back via `invoke("stdio_respond", ...)` per-request.
(async () => {
  console.log("[psyops-overlay] mount begin");
  try {
    registerPushHandler(handleRequest);
    console.log("[psyops-overlay] push handler registered, calling frontend_ready");
    await invoke("frontend_ready");
    console.log("[psyops-overlay] frontend_ready ok, calling current_mode");
    const mode = await invoke<Mode>("current_mode");
    console.log("[psyops-overlay] current_mode =", JSON.stringify(mode));
    if (mode !== null) {
      urlReporterUninstall = installSpaUrlReporter();
      onboardingHelpersUninstall = installOnboardingHelpers();
      appsTabHelperUninstall = installAppsTabHelper();
      appsPageHelpersUninstall = installAppsPageHelpers();
      createAppDialogHelpersUninstall = installCreateAppDialogHelpers();
      postCreateDialogHelpersUninstall = installPostCreateDialogHelpers();
      console.log("[psyops-overlay] helpers installed");
    }
  } catch (e) {
    console.error("[psyops-overlay] mount failed:", (e as Error)?.message ?? String(e));
  }
})();
