// Post-app-create dialog helpers — scrapes the three credentials
// X surfaces ONCE in the follow-up dialog after Create is clicked,
// ships each to Rust for atomic per-user storage, and shows a
// single "Click close" badge that's red until all three writes
// are acknowledged.
//
// The dialog has three labeled fields ("Consumer Key", "Secret
// Key", "Bearer Token") plus a Close button. We don't touch the
// page — we read the values, send them to Rust, and tell the
// user when it's safe to close.
//
// TOS posture: read-only DOM observation + invoke + render under
// our own shadow root. Forbidden APIs (`.value=`, `.click()`,
// `.dispatchEvent`, fetch) are unused.

import { invoke } from "@tauri-apps/api/core";
import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";

const HELPER_TEXT = "Click close";

// Field names match the SDK's `XAppCredentialField` snake-case
// wire form. Three of them; we send each independently via
// store_x_app_credential.
type Field = "consumer_key" | "secret_key" | "bearer_token";

const FIELD_LABELS: Record<Field, string> = {
  consumer_key: "Consumer Key",
  secret_key: "Secret Key",
  bearer_token: "Bearer Token",
};

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
// Dialog detection + scraping
// =================================================================

/**
 * Find the post-create credentials dialog. X labels it
 * something like "Keys & tokens" / "API Key generated" / "Save
 * your keys" — we try multiple known headings, plus a generic
 * "any dialog that contains all three field labels" fallback.
 */
function findPostCreateDialog(): HTMLElement | null {
  for (const d of document.querySelectorAll<HTMLElement>('[role="dialog"]')) {
    // Skip the previous create dialog explicitly.
    const heading = d
      .querySelector<HTMLElement>('h1, h2, h3, [role="heading"]')
      ?.textContent?.trim();
    if (heading === "Create New Client Application") continue;

    // Heuristic: dialog must contain all three field labels.
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

/** Walk labels in the dialog, find one whose text matches the
 *  expected field, follow its `for=` id (or fall back to a
 *  sibling input/code). Read `.value` (inputs) or `.textContent`
 *  (anything else). */
function scrapeFieldValue(dialog: HTMLElement, field: Field): string | null {
  const expected = FIELD_LABELS[field].toLowerCase();
  for (const label of dialog.querySelectorAll<HTMLLabelElement>("label")) {
    const t = label.textContent?.trim().toLowerCase() ?? "";
    if (t !== expected) continue;
    // 1. for-id paired.
    const id = label.getAttribute("for");
    if (id) {
      const el = dialog.querySelector<HTMLElement>(`#${cssEscape(id)}`);
      const v = readValue(el);
      if (v) return v;
    }
    // 2. Input descendant inside the label.
    const nested = label.querySelector<HTMLElement>("input, textarea, code");
    const nestedV = readValue(nested);
    if (nestedV) return nestedV;
    // 3. Sibling chain (the input often sits in the next
    //    element after the label).
    let sib = label.nextElementSibling as HTMLElement | null;
    while (sib) {
      const v = readValue(sib);
      if (v) return v;
      const inner = sib.querySelector<HTMLElement>("input, textarea, code");
      const innerV = readValue(inner);
      if (innerV) return innerV;
      sib = sib.nextElementSibling as HTMLElement | null;
    }
  }
  return null;
}

function readValue(el: HTMLElement | null | undefined): string | null {
  if (!el) return null;
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    return el.value?.trim() || null;
  }
  const t = el.textContent?.trim() ?? "";
  return t || null;
}

/** Find the Close button inside the dialog — text walk. */
function findCloseButton(dialog: HTMLElement): HTMLButtonElement | null {
  for (const b of dialog.querySelectorAll<HTMLButtonElement>("button")) {
    const t = b.textContent?.trim().toLowerCase() ?? "";
    if (
      t === "close" ||
      t === "done" ||
      t === "i have saved them" ||
      t.startsWith("i have saved") ||
      t === "got it"
    ) {
      return b;
    }
  }
  return null;
}

/** Minimal CSS.escape shim (handles common Radix-id chars). */
function cssEscape(s: string): string {
  if (typeof CSS !== "undefined" && CSS.escape) return CSS.escape(s);
  return s.replace(/([!"#$%&'()*+,./:;<=>?@[\\\]^`{|}~])/g, "\\$1");
}

// =================================================================
// Lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;

let cachedUserId: string | null = null;
let snapshotted: Record<Field, string> | null = null;
const storedSet = new Set<Field>();

function refreshUserId() {
  invoke<string | null>("current_user_id")
    .then((id) => {
      cachedUserId = id;
    })
    .catch(() => {
      cachedUserId = null;
    });
}

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

  widget = createHelperWidget({ text: HELPER_TEXT });
  widget.element.style.display = "none";
  shadow.appendChild(widget.element);

  document.body.appendChild(rootEl);

  refreshUserId();
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
  snapshotted = null;
  storedSet.clear();
  cachedUserId = null;
}

function tick() {
  if (!widget) return;
  const el = widget.element;
  const dialog = findPostCreateDialog();

  if (!dialog) {
    el.style.display = "none";
    // Reset state so a fresh dialog opens with a clean slate.
    snapshotted = null;
    storedSet.clear();
    rafId = requestAnimationFrame(tick);
    return;
  }

  // Anchor the badge to the LEFT of the Close button if we can
  // find one — otherwise just show it at the top-right of the
  // dialog as a fallback so the user still sees it.
  const close = findCloseButton(dialog);
  el.style.display = "";
  if (close) {
    const rect = close.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translateX(-100%) translateY(-50%)";
  } else {
    // Fallback: dialog top-right inside corner.
    const rect = dialog.getBoundingClientRect();
    el.style.top = `${rect.top + 12}px`;
    el.style.left = `${rect.right - 12}px`;
    el.style.transform = "translateX(-100%)";
  }

  // Width cap (same pattern as onboarding helpers).
  const VIEWPORT_MARGIN = 12;
  const MIN_WIDTH = 120;
  const MAX_WIDTH = 300;
  const leftEdgeRoom = close
    ? close.getBoundingClientRect().left - 8 - VIEWPORT_MARGIN
    : MAX_WIDTH;
  el.style.maxWidth = `${Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, leftEdgeRoom))}px`;

  // If we haven't snapshotted yet, try now.
  if (!snapshotted) {
    if (!cachedUserId) {
      // Re-query in case it became available since mount.
      refreshUserId();
    } else {
      const consumer_key = scrapeFieldValue(dialog, "consumer_key");
      const secret_key = scrapeFieldValue(dialog, "secret_key");
      const bearer_token = scrapeFieldValue(dialog, "bearer_token");
      if (consumer_key && secret_key && bearer_token) {
        snapshotted = { consumer_key, secret_key, bearer_token };
        const handle = cachedUserId;
        for (const field of Object.keys(snapshotted) as Field[]) {
          const value = snapshotted[field];
          invoke("store_x_app_credential", { handle, field, value })
            .then(() => {
              storedSet.add(field);
            })
            .catch(() => {
              // leave storedSet unchanged — badge stays red,
              // which is the right signal: "not yet safe to
              // close." A real failure would need debugging in
              // stdout (the Tauri command's error returns
              // there).
            });
        }
      }
    }
  }

  // Update badge state.
  if (snapshotted && storedSet.size === 3) {
    widget.setState("complete");
  } else {
    widget.setState("blocked");
  }

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
    // Refresh cached user-id whenever URL changes (typically
    // happens once on initial install + a few times during
    // navigation; cheap).
    if (rootEl) refreshUserId();
  });
  return () => {
    urlUnsubscribe?.();
    urlUnsubscribe = null;
    unmount();
  };
}
