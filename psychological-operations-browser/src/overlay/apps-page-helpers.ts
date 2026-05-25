// Apps-page helpers for `console.x.com/accounts/<id>/apps`.
//
// Two responsibilities while the user is on the Apps list:
//
//   1. Scrape the production-app count and ship it to Rust via
//      `set_production_app_count` so the panel can derive its
//      ClickCreateApp state. Counts apps grouped under an
//      `<h3>production</h3>` section; the empty case (no
//      production section at all) reports 0.
//
//   2. Pin a shared `helper-widget` pointer ("Click here") next
//      to the global "Create App" button, *visible only when the
//      production count is 0*. If the user already has a
//      production app, no pointer — they're past this step.
//
// Lifecycle: mounts on `/apps` (any sub-route), unmounts off
// /apps. On unmount, sends `count: null` so Rust drops the stale
// fact and the panel doesn't carry a Hidden state into a
// different page.
//
// TOS posture: same as the other overlay modules — read-only DOM
// observation, render under our own shadow root, optional
// clipboard write (not used here). Forbidden APIs (`.value=`,
// `.click()`, `.dispatchEvent`, fetch to x.com) are unused.

import { invoke } from "@tauri-apps/api/core";
import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";

const HELPER_TEXT = "Click here";

// =================================================================
// URL + DOM predicates
// =================================================================

function isOnAppsTab(url: string): boolean {
  try {
    const u = new URL(url);
    return (
      u.host === "console.x.com" &&
      /^\/accounts\/\d+\/apps(\/|$)/.test(u.pathname)
    );
  } catch {
    return false;
  }
}

/** Find the global "Create App" button at the top of the apps
 *  list page. There's exactly one. */
function findCreateAppButton(): HTMLButtonElement | null {
  for (const b of document.querySelectorAll<HTMLButtonElement>("button")) {
    if (b.textContent?.trim() === "Create App") return b;
  }
  return null;
}

/** Count production apps in the list. The page groups apps under
 *  `<section>` blocks headed by `<h3>` tags whose text is the
 *  app type (e.g. "production", "development", "standalone").
 *  When the user has zero apps of a given type the section
 *  isn't rendered at all — so a missing production section means
 *  zero production apps. */
function countProductionApps(): number {
  for (const h3 of document.querySelectorAll<HTMLHeadingElement>("h3")) {
    if (h3.textContent?.trim().toLowerCase() !== "production") continue;
    const section = h3.closest("section");
    if (!section) continue;
    // Each app row inside the section has an anchor like
    // `/accounts/<id>/apps/<app-id>`. Count those.
    let n = 0;
    for (const a of section.querySelectorAll<HTMLAnchorElement>("a[href]")) {
      const href = a.getAttribute("href") ?? "";
      if (/^\/accounts\/\d+\/apps\/\d+/.test(href)) n += 1;
    }
    return n;
  }
  return 0;
}

// =================================================================
// Mount / unmount lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;
let lastReportedCount: number | null | "uninit" = "uninit";

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_apps_page_helpers";
  Object.assign(rootEl.style, {
    position: "fixed",
    top: "0",
    left: "0",
    width: "0",
    height: "0",
    pointerEvents: "none",
    zIndex: "2147483600",
  } satisfies Partial<CSSStyleDeclaration>);

  const shadow = rootEl.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = HELPER_CSS;
  shadow.appendChild(style);

  widget = createHelperWidget({ text: HELPER_TEXT });
  widget.element.style.display = "none";
  shadow.appendChild(widget.element);

  document.body.appendChild(rootEl);
  lastReportedCount = "uninit";
  rafId = requestAnimationFrame(tick);
}

function unmount() {
  if (!rootEl) return;
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
  rootEl.remove();
  rootEl = null;
  widget = null;
  // Clear the fact in Rust so derive doesn't carry a stale count
  // (e.g. user navigates from /apps to /settings — Rust should
  // see "no count" and the panel falls back to ClickAppsTab,
  // not a stale Hidden).
  reportCount(null);
  lastReportedCount = "uninit";
}

function reportCount(count: number | null) {
  if (lastReportedCount !== "uninit" && lastReportedCount === count) return;
  lastReportedCount = count;
  invoke("set_production_app_count", { count }).catch(() => {
    // best-effort
  });
}

function tick() {
  if (!widget) return;

  // 1. Scrape production count; push to Rust if it moved.
  const count = countProductionApps();
  reportCount(count);

  // 2. Position Create App pointer (visible only when count is 0).
  const btn = findCreateAppButton();
  const el = widget.element;
  if (!btn || count > 0) {
    el.style.display = "none";
  } else {
    el.style.display = "";
    // Anchor to the LEFT of the button. `translateX(-100%)`
    // pins the helper's right edge at the chosen left
    // coordinate, so we don't have to measure helper width.
    const rect = btn.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translateX(-100%) translateY(-50%)";
  }
  rafId = requestAnimationFrame(tick);
}

// =================================================================
// Public entry point
// =================================================================
let urlUnsubscribe: (() => void) | null = null;

/**
 * Install the apps-page helpers (production count + Create App
 * pointer). Returns an uninstall closure. Mounts/unmounts
 * automatically based on URL — only active on the Apps tab.
 */
export function installAppsPageHelpers(): () => void {
  urlUnsubscribe = subscribeUrl((url) => {
    if (isOnAppsTab(url)) {
      mount();
    } else {
      unmount();
    }
  });
  return () => {
    urlUnsubscribe?.();
    urlUnsubscribe = null;
    unmount();
  };
}
