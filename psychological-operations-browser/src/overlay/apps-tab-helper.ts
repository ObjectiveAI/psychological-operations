// Sidebar pointer for the "Apps" tab on `console.x.com`.
//
// Single "Click here" badge anchored to the right of the Apps
// link in the sidebar. Hidden whenever:
//   - the user is already on the Apps tab
//     (`/accounts/<id>/apps` and below), OR
//   - the Apps link isn't on the page (e.g. /onboarding, signed-
//     out screens, anywhere with no sidebar)
//
// Visuals come from the shared `helper-widget` module so this
// matches the onboarding-helpers look-and-feel.
//
// TOS posture is the same as onboarding-helpers: read-only DOM
// observation + rendering under our own shadow root. Forbidden
// APIs (`.value=`, `.click()`, `.dispatchEvent`, fetch to x.com)
// are unused — grep this file if reviewing.

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

/**
 * True when the current page is *on or under* the Apps tab —
 * either the apps list (`/accounts/<id>/apps`) or a specific app
 * page (`/accounts/<id>/apps/<app-id>` and its sub-routes).
 */
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

/**
 * Find the "Apps" link in the sidebar. The exact href is
 * `/accounts/<account-id>/apps` (no trailing segments — those
 * would be a *specific* app's page) and the link text is
 * literally "Apps".
 */
function findAppsLink(): HTMLAnchorElement | null {
  for (const a of document.querySelectorAll<HTMLAnchorElement>("a[href]")) {
    const href = a.getAttribute("href") ?? "";
    if (
      /^\/accounts\/\d+\/apps$/.test(href) &&
      a.textContent?.trim() === "Apps"
    ) {
      return a;
    }
  }
  return null;
}

// =================================================================
// Mount lifecycle — single widget, always mounted, toggled via
// display:none from the tick loop based on URL + link presence.
// =================================================================
let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_apps_tab_helper";
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

  widget = createHelperWidget({ text: HELPER_TEXT, arrow: "left" });
  shadow.appendChild(widget.element);
  // Start hidden — first tick decides if/where to show.
  widget.element.style.display = "none";

  document.body.appendChild(rootEl);
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
}

function tick() {
  if (!widget) return;
  const el = widget.element;
  const link = findAppsLink();
  const show = !!link && !isOnAppsTab(location.href);

  if (!show || !link) {
    el.style.display = "none";
  } else {
    el.style.display = "";
    // Anchor to the RIGHT of the link. The sidebar hugs the
    // viewport's left edge — anchoring left would clip
    // off-screen. Vertically center against the link's row.
    const rect = link.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.right + 8}px`;
    el.style.transform = "translateY(-50%)";
  }
  rafId = requestAnimationFrame(tick);
}

// =================================================================
// Public entry point
// =================================================================

/**
 * Install the apps-tab pointer. Returns an uninstall closure.
 * Always-mounted-but-conditionally-hidden — the tick loop is
 * cheap (one selector walk + one rect read per frame) and we'd
 * rather avoid mount/unmount churn on every URL change.
 *
 * The URL subscription is currently a no-op hook; the tick loop
 * already polls `location.href` so visibility flips correctly
 * without it. Subscribed anyway so we can hang per-route logic
 * here later without re-plumbing the install.
 */
export function installAppsTabHelper(): () => void {
  mount();
  urlUnsubscribe = subscribeUrl(() => {
    // Visibility is driven entirely by the tick loop —
    // intentional placeholder.
  });
  return () => {
    urlUnsubscribe?.();
    urlUnsubscribe = null;
    unmount();
  };
}
