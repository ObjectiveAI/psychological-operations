import { invoke } from "@tauri-apps/api/core";

// Patches `history.pushState` / `history.replaceState` and listens
// for `popstate` / `hashchange` so we catch SPA route changes inside
// x.com — the native `WebviewWindowBuilder::on_navigation` callback
// on the Rust side only fires for full-page navigations.
//
// Reports the initial URL once after install so the first
// `Output::Url` line lands even when on_navigation may have already
// fired by the time the bundle runs.
export function installSpaUrlReporter(): void {
  const report = () => {
    invoke("report_url", { url: location.href }).catch(() => {
      // best-effort — overlay must not crash because IPC failed.
    });
  };

  const wrap = (key: "pushState" | "replaceState") => {
    const original = history[key];
    history[key] = function (
      this: History,
      ...args: Parameters<typeof original>
    ) {
      const result = original.apply(this, args);
      report();
      return result;
    } as typeof original;
  };
  wrap("pushState");
  wrap("replaceState");

  window.addEventListener("popstate", report);
  window.addEventListener("hashchange", report);

  if (
    document.readyState === "interactive" ||
    document.readyState === "complete"
  ) {
    report();
  } else {
    document.addEventListener("DOMContentLoaded", report, { once: true });
  }
}
