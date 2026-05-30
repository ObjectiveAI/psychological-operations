// Pointer for the "Settings" tab on an individual app's page
// (`console.x.com/accounts/<id>/apps/<app-id>`).
//
// Fires only on the strict app-overview URL — sub-routes like
// `/apps/<id>/settings` (where the user lands after clicking
// through) are skipped so we don't keep nagging.
//
// Pointer gates on `isPanelCondition("click_settings")`. Rust's
// derive sets that condition when:
//   - creds_complete == Some(true)   (first three on disk)
//   - access_tokens_complete != Some(true)
//   - URL strictly matches /accounts/<id>/apps/<app-id>
//
// Visual: badge sits to the LEFT of the Settings element with
// the triangle pointing right at it — same shape as the
// Create App pointer.
//
// TOS posture: read-only DOM observation + render under our own
// shadow root. Forbidden APIs unused.

import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";
import { isPanelCondition } from "./panel-state";

const HELPER_TEXT = "Click here";

// =================================================================
// URL gate — strict app overview only
// =================================================================
function isOnAppPage(url: string): boolean {
  try {
    const u = new URL(url);
    return (
      u.host === "console.x.com" &&
      /^\/accounts\/\d+\/apps\/[^/]+\/?$/.test(u.pathname)
    );
  } catch {
    return false;
  }
}

function isVisible(el: HTMLElement): boolean {
  const r = el.getBoundingClientRect();
  return r.width >= 1 && r.height >= 1;
}

function directText(el: HTMLElement): string {
  let s = "";
  for (const n of el.childNodes) {
    if (n.nodeType === Node.TEXT_NODE) s += n.textContent ?? "";
  }
  return s.trim();
}

/** Find the Settings nav target. Multi-strategy:
 *    1. Common interactive roles whose **whole** textContent is
 *       "Settings".
 *    2. Aria-label / title attribute exactly "Settings".
 *    3. Any element whose **direct** (own text-node only) text
 *       is "Settings" — typically a `<span>` inside a tab —
 *       climbing up to the nearest clickable ancestor.
 *  First visible hit wins. */
function findSettingsTarget(): { el: HTMLElement; via: string } | null {
  for (const sel of [
    "a",
    "button",
    '[role="tab"]',
    '[role="menuitem"]',
    '[role="button"]',
  ]) {
    for (const el of document.querySelectorAll<HTMLElement>(sel)) {
      if ((el.textContent ?? "").trim() !== "Settings") continue;
      if (!isVisible(el)) continue;
      return { el, via: `text:${sel}` };
    }
  }
  for (const sel of ['[aria-label="Settings"]', '[title="Settings"]']) {
    for (const el of document.querySelectorAll<HTMLElement>(sel)) {
      if (!isVisible(el)) continue;
      return { el, via: `attr:${sel}` };
    }
  }
  for (const el of document.querySelectorAll<HTMLElement>("*")) {
    if (directText(el) !== "Settings") continue;
    if (!isVisible(el)) continue;
    const clickable = el.closest<HTMLElement>(
      'a, button, [role="tab"], [role="button"], [role="menuitem"]',
    );
    return { el: clickable ?? el, via: "directText" };
  }
  return null;
}

// =================================================================
// Lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_app_page_helpers";
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

  widget = createHelperWidget({ text: HELPER_TEXT, arrow: "right" });
  widget.element.style.display = "none";
  shadow.appendChild(widget.element);

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
  const panelOk = isPanelCondition("click_settings");
  if (!panelOk) {
    el.style.display = "none";
    rafId = requestAnimationFrame(tick);
    return;
  }
  const found = findSettingsTarget();
  if (found) {
    el.style.display = "";
    widget.setText("Click here");
    // Badge sits LEFT of the Settings target; the widget's own
    // `arrow: "right"` makes the triangle point back at it.
    const rect = found.el.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translateX(-100%) translateY(-50%)";
  } else {
    // TEMP: Settings element not found — surface a diagnostic
    // badge in the top-right corner so we can tune the finder
    // against the live DOM. Remove once positioning is right.
    el.style.display = "";
    widget.setText("Settings target not found — share a screenshot of nearby UI");
    el.style.top = `12px`;
    el.style.left = `${window.innerWidth - 12}px`;
    el.style.transform = "translateX(-100%)";
  }
  rafId = requestAnimationFrame(tick);
}

// =================================================================
// Public entry point
// =================================================================
export function installAppPageHelpers(): () => void {
  urlUnsubscribe = subscribeUrl((url) => {
    if (isOnAppPage(url)) {
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
