// Post-app-create dialog helpers — ships the full document HTML
// to Rust whenever a dialog is open, lets Rust handle ALL the
// parsing + extraction + storage. The overlay's job here is
// purely "page → host" bytes pipe + the red→green badge.
//
// =================================================================
// TOS posture — overlay does zero data analysis:
// =================================================================
//
//   The overlay's only DOM reads are:
//     - `[role="dialog"]` presence check
//     - Locating a Close-style button to anchor the badge
//     - `document.documentElement.outerHTML` (raw bytes ship)
//
//   No label-text walks, no input.value reads, no parsing. All
//   credential extraction happens Rust-side
//   (`crate::post_create_dialog`). This keeps the in-page
//   surface area maximally legible to a reviewer asking "is
//   this thing reading the user's sensitive data?" — the answer
//   is "it ships raw HTML, the host process handles the rest."

import { invoke } from "./ipc";
import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";

const HELPER_TEXT = "Click close";
const RESEND_INTERVAL_MS = 2_000;

// =================================================================
// URL gate
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

// =================================================================
// Minimal DOM probes — these are NOT parsing; just structural.
// =================================================================

/**
 * Find the post-create credentials dialog specifically — the one
 * with Consumer Key / Secret Key / Bearer Token. Matching on any
 * `[role="dialog"]` would also match the *Create New Client
 * Application* dialog (Name + Environment + Create), which is a
 * different dialog handled by `create-app-dialog-helpers`.
 *
 * Checking for the three static field-label strings is a
 * structural distinguisher — not parsing of credential values
 * (which still happens Rust-side after the full HTML ships).
 */
function findPostCreateDialog(): HTMLElement | null {
  for (const d of document.querySelectorAll<HTMLElement>('[role="dialog"]')) {
    const text = (d.textContent ?? "").toLowerCase();
    if (
      text.includes("consumer key") &&
      text.includes("secret key") &&
      text.includes("bearer token")
    ) {
      return d;
    }
  }
  return null;
}

function findCloseButton(dialog: HTMLElement): HTMLButtonElement | null {
  for (const b of dialog.querySelectorAll<HTMLButtonElement>("button")) {
    const t = b.textContent?.trim().toLowerCase() ?? "";
    if (
      t === "close" ||
      t === "done" ||
      t === "got it" ||
      t.startsWith("i have saved") ||
      t.startsWith("i've saved") ||
      t.startsWith("ok")
    ) {
      return b;
    }
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

let captured = false;
let lastSendAt = 0;
let inFlight = false;

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_post_create_dialog_helpers";
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
  captured = false;
  lastSendAt = 0;
  inFlight = false;
}

function tick() {
  if (!widget) return;
  const el = widget.element;
  const dialog = findPostCreateDialog();

  if (!dialog) {
    el.style.display = "none";
    // Reset capture state — a future dialog is a fresh capture.
    captured = false;
    inFlight = false;
    rafId = requestAnimationFrame(tick);
    return;
  }

  // Anchor the badge to the LEFT of a Close-style button when
  // we can find one, else top-right of the dialog as a fallback
  // so the user still sees the state.
  el.style.display = "";
  const close = findCloseButton(dialog);
  if (close) {
    const rect = close.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translateX(-100%) translateY(-50%)";
  } else {
    const rect = dialog.getBoundingClientRect();
    el.style.top = `${rect.top + 12}px`;
    el.style.left = `${rect.right - 12}px`;
    el.style.transform = "translateX(-100%)";
  }

  // Width cap (matches the other helpers; wrap on narrow viewports).
  const VIEWPORT_MARGIN = 12;
  const MIN_WIDTH = 120;
  const MAX_WIDTH = 300;
  const leftEdgeRoom = close
    ? close.getBoundingClientRect().left - 8 - VIEWPORT_MARGIN
    : MAX_WIDTH;
  el.style.maxWidth = `${Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, leftEdgeRoom))}px`;

  // Ship the full document HTML to Rust if:
  //   - we haven't already captured (storedSet === 3 on Rust side)
  //   - no invoke is in-flight (avoid stacking pending requests)
  //   - at least RESEND_INTERVAL_MS since the last send (avoid
  //     hammering Rust + the IPC layer with the full document
  //     every frame)
  const now = performance.now();
  if (!captured && !inFlight && now - lastSendAt >= RESEND_INTERVAL_MS) {
    lastSendAt = now;
    inFlight = true;
    const html = document.documentElement.outerHTML;
    invoke<number>("process_post_create_html", { html })
      .then((stored) => {
        if (stored >= 3) captured = true;
      })
      .catch(() => {
        // Most common: "no user_id yet" before the cookies
        // watcher has run a snapshot. We'll retry next interval.
      })
      .finally(() => {
        inFlight = false;
      });
  }

  widget.setState(captured ? "complete" : "blocked");
  rafId = requestAnimationFrame(tick);
}

// =================================================================
// Public entry point
// =================================================================
export function installPostCreateDialogHelpers(): () => void {
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
