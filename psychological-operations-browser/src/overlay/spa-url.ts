import { invoke } from "@tauri-apps/api/core";

// Reports `location.href` to the Rust side immediately and on every
// SPA route change (history.pushState / replaceState / popstate /
// hashchange). Returns an uninstall closure that restores the
// original History methods and detaches the listeners — the X-App
// handler holds onto this so subsequent mode resets can stop the
// reporter cleanly before navigating.
export function installSpaUrlReporter(): () => void {
  // Dedup — page scripts often call replaceState/pushState with the
  // same URL during init, and we don't want to spam stdout with
  // duplicate `Output::Url` lines.
  let last: string | undefined;
  const report = () => {
    if (location.href === last) return;
    last = location.href;
    invoke("report_url", { url: location.href }).catch(() => {
      // best-effort — overlay must not crash because IPC failed.
    });
  };

  const originals: Partial<Record<"pushState" | "replaceState", History[keyof History]>> = {};

  const wrap = (key: "pushState" | "replaceState") => {
    const original = history[key];
    originals[key] = original;
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

  // Report the initial URL synchronously — the overlay is mounted
  // by the time we get here, and `location.href` is already correct.
  report();

  return () => {
    if (originals.pushState) {
      history.pushState = originals.pushState as History["pushState"];
    }
    if (originals.replaceState) {
      history.replaceState = originals.replaceState as History["replaceState"];
    }
    window.removeEventListener("popstate", report);
    window.removeEventListener("hashchange", report);
  };
}
