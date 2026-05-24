import { invoke } from "@tauri-apps/api/core";

// One-time wrapping of the History methods + listeners for
// popstate/hashchange. Done lazily on first `subscribeUrl` call so
// importing this module has no side-effects. The wrappers stay
// installed for the life of the JS context — when the content
// webview does a full-document navigation, the context is torn
// down anyway, which takes the wrappers with it.

type Subscriber = (url: string) => void;

let installed = false;
let lastFired: string | undefined;
const subscribers = new Set<Subscriber>();

function fire() {
  const url = location.href;
  // Dedup — page scripts often call replaceState/pushState with the
  // same URL during init, and there's no reason to re-do work on a
  // same-URL change.
  if (url === lastFired) return;
  lastFired = url;
  for (const cb of subscribers) {
    try {
      cb(url);
    } catch {
      // Subscribers must not break the dispatch loop for each
      // other. Swallow.
    }
  }
}

function installOnce() {
  if (installed) return;
  installed = true;

  const wrap = (key: "pushState" | "replaceState") => {
    const original = history[key];
    history[key] = function (
      this: History,
      ...args: Parameters<typeof original>
    ) {
      const result = original.apply(this, args);
      fire();
      return result;
    } as typeof original;
  };
  wrap("pushState");
  wrap("replaceState");
  window.addEventListener("popstate", fire);
  window.addEventListener("hashchange", fire);
}

/**
 * Subscribe to URL changes. Returns an unsubscribe closure.
 * Fires the callback immediately with the current URL on
 * registration so subscribers can hydrate state without waiting
 * for the next nav.
 */
export function subscribeUrl(cb: Subscriber): () => void {
  installOnce();
  subscribers.add(cb);
  try {
    cb(location.href);
  } catch {
    // best-effort
  }
  return () => {
    subscribers.delete(cb);
  };
}

/**
 * Reports `location.href` to the Rust side immediately and on
 * every SPA route change. Returns an uninstall closure — held by
 * the X-App handler so it can stop reporting before navigating on
 * an `x_app` re-dispatch. Implemented as a `subscribeUrl`
 * subscriber so any other module (e.g. the onboarding-helpers
 * mount/unmount logic) can hook into the same URL stream without
 * re-wrapping History methods.
 */
export function installSpaUrlReporter(): () => void {
  return subscribeUrl((url) => {
    invoke("report_url", { url }).catch(() => {
      // best-effort — overlay must not crash because IPC failed.
    });
  });
}
