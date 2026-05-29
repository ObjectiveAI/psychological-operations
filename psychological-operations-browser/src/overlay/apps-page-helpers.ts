// Apps-page helpers for `console.x.com/accounts/<id>/apps`.
//
// Three responsibilities while the user is on the Apps list:
//
//   1. Scrape the production-app count and ship it to Rust via
//      `set_production_app_count` so the panel can pick between
//      `ClickCreateApp` (zero apps) and `ClickProductionApp`
//      (one+ apps) when the user has the first three creds but
//      not the access tokens.
//
//   2. Pin a shared `helper-widget` pointer ("Click here") next
//      to the global "Create App" button, visible only when the
//      panel is in `ClickCreateApp` state.
//
//   3. Pin a second `helper-widget` pointer next to the *first*
//      production app row in the list, visible only when the
//      panel is in `ClickProductionApp` state — i.e. the user
//      already has the first three creds and needs to drill into
//      their app to capture the access-token pair.
//
// `isCreateAppDialogOpen` predicate is exported so
// `create-app-dialog-helpers` can re-import it (the predicate
// naturally belongs here — it's about the apps page, not the
// dialog).
//
// Lifecycle: mounts on `/apps` (any sub-route), unmounts off
// /apps. On unmount, sends `count: null` so Rust drops the stale
// fact and the panel doesn't carry it into a different page.
//
// TOS posture: same as the other overlay modules — read-only DOM
// observation, render under our own shadow root. Forbidden APIs
// (`.value=`, `.click()`, `.dispatchEvent`, fetch to x.com) are
// unused.

import { invoke } from "./ipc";
import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";
import { isPanelCondition } from "./panel-state";

const HELPER_TEXT = "Click here";

// =================================================================
// URL + DOM predicates
// =================================================================

/** Strict apps-list check — only `/apps[/]?`, not individual
 *  app sub-routes. The count reporter + both pointers only make
 *  sense on the list page; on an individual app's page the
 *  production section / Create App button don't exist, and a
 *  stale scrape there would flip the panel to ClickCreateApp.
 *  Distinct from `apps-tab-helper`'s broad isOnAppsTab, which
 *  intentionally includes sub-routes for "user is in the Apps
 *  area" semantics. */
function isOnAppsList(url: string): boolean {
  try {
    const u = new URL(url);
    return (
      u.host === "console.x.com" &&
      /^\/accounts\/\d+\/apps\/?$/.test(u.pathname)
    );
  } catch {
    return false;
  }
}

/** Find the global "Create App" button at the top of the apps
 *  list page. There's exactly one — scoped to exclude buttons
 *  inside dialogs (the Create dialog also has a Create-shaped
 *  button). */
function findCreateAppButton(): HTMLButtonElement | null {
  for (const b of document.querySelectorAll<HTMLButtonElement>("button")) {
    if (b.textContent?.trim() !== "Create App") continue;
    if (b.closest('[role="dialog"]')) continue;
    return b;
  }
  return null;
}

/** True iff the "Create New Client Application" dialog is open on
 *  the page. Exact heading-text match — Radix dialogs render the
 *  heading as an `<h2>` inside `[role="dialog"]`. Exported so
 *  `create-app-dialog-helpers` can share the predicate. */
export function isCreateAppDialogOpen(): boolean {
  for (const d of document.querySelectorAll('[role="dialog"]')) {
    const heading = d.querySelector<HTMLElement>(
      'h1, h2, h3, [role="heading"]',
    );
    const text = heading?.textContent?.trim() ?? "";
    if (text === "Create New Client Application") return true;
  }
  return false;
}

/** All anchor elements inside the production `<section>` that
 *  point at a specific app (`/accounts/<digits>/apps/<digits>`).
 *  Returned in document order so [0] is the first row. Empty
 *  array when the user has no production apps (the section is
 *  absent in that case). */
function findProductionAppLinks(): HTMLAnchorElement[] {
  const out: HTMLAnchorElement[] = [];
  for (const h3 of document.querySelectorAll<HTMLHeadingElement>("h3")) {
    if (h3.textContent?.trim().toLowerCase() !== "production") continue;
    const section = h3.closest("section");
    if (!section) continue;
    for (const a of section.querySelectorAll<HTMLAnchorElement>("a[href]")) {
      const href = a.getAttribute("href") ?? "";
      if (/^\/accounts\/\d+\/apps\/\d+/.test(href)) out.push(a);
    }
    return out;
  }
  return out;
}

/** Walk up from a `/apps/<id>` link to the visible "row" / card
 *  container it lives inside, so the pointer anchors to the
 *  bigger UI element instead of just the small text link.
 *
 *  Strategy: track the widest right-edge seen on the way up
 *  (capped at the production `<section>`), and return whichever
 *  ancestor pushed the right edge out the most. That covers the
 *  common Tailwind / Radix card pattern where the visible card
 *  is somewhere a couple levels above the inner text link, with
 *  every wrapping div between cumulatively contributing padding /
 *  flex layout to the card's right edge.
 *
 *  Stops at <section> — that's the column the rows live in, not
 *  the row itself. */
function findRowContainer(anchor: HTMLAnchorElement): HTMLElement {
  let el: HTMLElement = anchor;
  let bestEl: HTMLElement = anchor;
  let bestRight = anchor.getBoundingClientRect().right;
  for (let i = 0; i < 8; i++) {
    if (!el.parentElement) break;
    if (el.parentElement.tagName === "SECTION") break;
    el = el.parentElement;
    const r = el.getBoundingClientRect();
    if (r.right > bestRight + 0.5) {
      bestRight = r.right;
      bestEl = el;
    }
  }
  return bestEl;
}

function countProductionApps(): number {
  return findProductionAppLinks().length;
}

// =================================================================
// Lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
let createBtnWidget: HelperWidget | null = null;
let prodAppWidget: HelperWidget | null = null;
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;
let lastReportedCount: number | "uninit" = "uninit";
/// Consecutive ticks where the production-app scrape returned 0.
/// Non-zero counts are trusted immediately (a section + anchors
/// did render). Zero counts are ambiguous — could be "page still
/// fetching the apps list" or "user genuinely has zero apps".
/// We require ~500ms of consistent 0s before reporting one to
/// Rust to suppress a flash to ClickCreateApp on initial mount.
let zeroStreak = 0;
/// One-shot log of the chosen row container's tag/classes/rect,
/// so we can tune `findRowContainer` against the actual markup
/// without needing to inspect via devtools. Resets on `mount()`.
let loggedRowChoice = false;

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

  createBtnWidget = createHelperWidget({ text: HELPER_TEXT, arrow: "right" });
  createBtnWidget.element.style.display = "none";
  shadow.appendChild(createBtnWidget.element);

  // Anchored to the RIGHT of the row, so the triangle points LEFT
  // back at the row.
  prodAppWidget = createHelperWidget({ text: HELPER_TEXT, arrow: "left" });
  prodAppWidget.element.style.display = "none";
  shadow.appendChild(prodAppWidget.element);

  document.body.appendChild(rootEl);
  lastReportedCount = "uninit";
  zeroStreak = 0;
  loggedRowChoice = false;
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
  createBtnWidget = null;
  prodAppWidget = null;
  // Clear Rust's count fact so a stale value can't drive
  // ClickCreateApp / ClickProductionApp on a different page.
  if (lastReportedCount !== null) {
    invoke("set_production_app_count", { count: null }).catch(() => {});
    lastReportedCount = null as unknown as "uninit";
  }
}

function tick() {
  if (!rootEl || !createBtnWidget || !prodAppWidget) return;

  // -- Count report --------------------------------------------------
  // Don't double-count while a dialog is open (the create-app
  // dialog can fade the underlying list out of the DOM in some
  // states; whatever count we'd get isn't authoritative).
  // Also require the Create App button — its presence is a
  // cheap proxy for "the page header has rendered", which the
  // React app does synchronously after mount but before the
  // async apps-fetch resolves. Without this gate, the very
  // first tick can scrape 0 and flip Rust to ClickCreateApp.
  const headerReady = !!findCreateAppButton();
  if (!isCreateAppDialogOpen() && headerReady) {
    const count = countProductionApps();
    if (count === 0) {
      // Debounce 0 only — non-zero scrapes are always trustable
      // (the production section exists if we found a row).
      zeroStreak += 1;
    } else {
      zeroStreak = 0;
    }
    const trustable = count !== 0 || zeroStreak >= 30;
    if (trustable && count !== lastReportedCount) {
      lastReportedCount = count;
      invoke("set_production_app_count", { count }).catch(() => {});
    }
  }

  // -- Create App pointer -------------------------------------------
  const createEl = createBtnWidget.element;
  const createBtn = findCreateAppButton();
  const showCreate =
    !!createBtn && isPanelCondition("click_create_app");
  if (!showCreate || !createBtn) {
    createEl.style.display = "none";
  } else {
    createEl.style.display = "";
    const rect = createBtn.getBoundingClientRect();
    createEl.style.top = `${rect.top + rect.height / 2}px`;
    createEl.style.left = `${rect.left - 8}px`;
    createEl.style.transform = "translateX(-100%) translateY(-50%)";
  }

  // -- Production app pointer ---------------------------------------
  const prodEl = prodAppWidget.element;
  const prodLinks = findProductionAppLinks();
  const firstProd = prodLinks[0] ?? null;
  const showProd =
    !!firstProd && isPanelCondition("click_production_app");
  if (!showProd || !firstProd) {
    prodEl.style.display = "none";
  } else {
    prodEl.style.display = "";
    // Horizontal: anchor to the right edge of the broader
    // card / row container so the badge clears the visible
    // card. Vertical: anchor to the inner anchor's center —
    // if `findRowContainer` over-picks (e.g. lands on a
    // wrapping list), vertical alignment still tracks the
    // actual row.
    const row = findRowContainer(firstProd);
    const rowRect = row.getBoundingClientRect();
    const anchorRect = firstProd.getBoundingClientRect();
    prodEl.style.top = `${anchorRect.top + anchorRect.height / 2}px`;
    prodEl.style.left = `${rowRect.right + 8}px`;
    prodEl.style.transform = "translateY(-50%)";
    if (!loggedRowChoice) {
      loggedRowChoice = true;
      console.log(
        "[psyops-overlay] row container chosen for prod-app pointer:",
        row.tagName,
        Array.from(row.classList).slice(0, 4).join(" "),
        "| rect:", JSON.stringify({
          left: Math.round(rowRect.left),
          right: Math.round(rowRect.right),
          width: Math.round(rowRect.width),
          height: Math.round(rowRect.height),
        }),
        "| anchor rect:", JSON.stringify({
          left: Math.round(anchorRect.left),
          right: Math.round(anchorRect.right),
          width: Math.round(anchorRect.width),
        }),
      );
    }
  }

  rafId = requestAnimationFrame(tick);
}

// =================================================================
// Public entry point
// =================================================================

export function installAppsPageHelpers(): () => void {
  urlUnsubscribe = subscribeUrl((url) => {
    if (isOnAppsList(url)) {
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
