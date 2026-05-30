// PsyopRead overlay helper.
//
// Installed by `main.tsx` only when `current_mode().type === "psyop_read"`.
// While installed, ships full-page HTML to Rust on a 2s rAF tick
// (skipping while a prior invoke is still in flight). Rust does
// the parsing + dedup + tweet-ID emit; we just hand it the
// snapshot.
//
// Matches the rAF + throttle + in-flight pattern from
// `post-create-dialog-helpers.ts`. The returned count is ignored
// on the JS side — the panel state from Rust (via
// `window.__psyops_set_panel`) carries the visible counter.

import { invoke } from "./ipc";

const SEND_INTERVAL_MS = 2000;

export function installPsyopReadHelpers(): () => void {
  let rafHandle: number | null = null;
  let stopped = false;
  let inFlight = false;
  let lastSendAt = 0;

  const tick = () => {
    if (stopped) return;
    rafHandle = window.requestAnimationFrame(tick);
    if (inFlight) return;
    const now = performance.now();
    if (now - lastSendAt < SEND_INTERVAL_MS) return;
    // Guard against the very first tick before the page has
    // any body — outerHTML on an empty document is fine, but
    // not worth shipping.
    if (!document.body) return;
    const html = document.documentElement.outerHTML;
    lastSendAt = now;
    inFlight = true;
    invoke<number>("process_read_html", { html })
      .catch((e) => {
        console.warn("[psyops-overlay] process_read_html failed:", e);
      })
      .finally(() => {
        inFlight = false;
      });
  };
  rafHandle = window.requestAnimationFrame(tick);

  return () => {
    stopped = true;
    if (rafHandle !== null) {
      window.cancelAnimationFrame(rafHandle);
      rafHandle = null;
    }
  };
}
