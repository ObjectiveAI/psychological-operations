// Reply/quote delivery overlay — stage-2 UI + detection state machine.
//
// Driven by the Rust delivery driver (`src-tauri/src/deliver.rs`), which
// navigates the agent's logged-in x.com session to a target tweet's status
// page and then calls `window.__psyops_deliver(item)` once per item. This
// module walks the operator through posting one reply/quote and reports a
// single outcome back via `invoke("deliver_report", { tweet_id, kind,
// status })` — `"done"` on a confirmed post, `"skip"` on timeout / give-up.
// The Rust driver's per-item oneshot is keyed on `(tweet_id, kind)`.
//
// FLOW (per item):
//
//   State A — TWEET PAGE (URL is not /compose/post)
//     reply : "Click here" badge next to the reply button.
//     quote : "Click here" next to the repost button; once the repost menu
//             opens, re-anchor to its "Quote" entry.
//   --(URL → /compose/post)-->
//   State B — COMPOSER (URL is /compose/post)
//     • copy widget ("Paste here", copyText = item.content) by the body.
//     • "Click here" by the Post button: RED until the body matches, GREEN
//       once it does.
//   --(URL leaves /compose/post)--> done | recover
//
// RESOLUTION — the CONJUNCTION of three independent signals (critical):
//   1. the Post button was actually CLICKED (observed via a capturing click
//      listener — a genuine user gesture, read-only),
//   2. the body was GREEN (matched) at the instant of that click, and
//   3. the URL then LEFT /compose/post (X mutates history synthetically;
//      `subscribeUrl` wraps pushState/popstate).
//   Only all three ⇒ "done". Any two without the third ⇒ recover (and a
//   green→non-green edit after an armed click disarms it). This keeps an
//   accidental "cancel while green" from being mistaken for a post.
//
// TOS posture matches the other overlay helpers: read-only DOM observation
// + rendering under our own (closed) shadow root, plus a passive click
// listener on a real user gesture. No `.value=`, no `.click()`, no
// `.dispatchEvent`, no page network calls — the operator drives the post.
//
// ITERATION POINTS (refined against the live DOM via the dev bridge — no
// x.com content selectors existed in this codebase before this flow):
// every `[data-testid]` below, the repost-menu "Quote" locator, and the
// match normalization in `bodyMatches()`.

import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";
import { invoke } from "./ipc";

/** Mirror of the Rust `DeliverItem` (sdk browser/deliver.rs). */
export type DeliverItem = {
  tweet_id: string;
  agent: string;
  content: string;
  kind: string; // "reply" | "quote"
};

// ---- ITERATION POINTS: x.com selectors -------------------------------
const REPLY_BUTTON = '[data-testid="reply"]';
const REPOST_BUTTON = '[data-testid="retweet"]';
// The composer contenteditable (status-page inline + /compose modal).
const COMPOSER = '[data-testid="tweetTextarea_0"]';
// The post/submit button inside the composer.
const POST_BUTTON =
  '[data-testid="tweetButton"],[data-testid="tweetButtonInline"]';
// ----------------------------------------------------------------------

// No wall-clock timeout: delivery is operator-actuated, so the flow waits
// indefinitely for the operator to post (auto-resolve) or skip. A timer
// would only punish a human for taking their time.
//
// ===================================================================
// Per-item flow state
// ===================================================================
let current: DeliverItem | null = null;
let reported = false;

// URL phase: are we currently on /compose/post (State B)?
let onCompose = false;
// Latched at the moment the Post button is clicked: true iff the body
// matched at that instant. The URL-leave then resolves iff this is set.
let armed = false;
// Set once the 3-check resolution fires (post detected as sent). We do NOT
// report "done" yet — the bottom button becomes "Continue" and the operator
// confirms X actually posted (its "Post sent!" toast can lag) before we
// advance. Continue -> report("done").
let sent = false;

let urlUnsub: (() => void) | null = null;
let rafId: number | null = null;

// DOM
let rootEl: HTMLDivElement | null = null;
let clickAction: HelperWidget | null = null; // State A "Click here"
let copyBody: HelperWidget | null = null; // State B "Paste here"
let clickPost: HelperWidget | null = null; // State B "Click here" (Post)
// Bottom button: "Skip this one" before send, "Continue" after.
let bottomBtn: HTMLButtonElement | null = null;

// ===================================================================
// URL predicate
// ===================================================================
function isComposeUrl(url: string): boolean {
  try {
    const u = new URL(url);
    // x.com mutates to /compose/post (sometimes with a query) when the
    // composer opens. (Iteration point.)
    return u.pathname.startsWith("/compose/post");
  } catch {
    return false;
  }
}

// ===================================================================
// Match test — does the composer body equal the expected content?
// ===================================================================
function bodyMatches(): boolean {
  if (!current) return false;
  const composer = document.querySelector<HTMLElement>(COMPOSER);
  if (!composer) return false;
  // contenteditable → textContent (the X-App convention; `.value` is for
  // <input>/<textarea>). (Iteration point: normalization.)
  const text = (composer.textContent ?? "").trim();
  return text.length > 0 && text === current.content.trim();
}

// ===================================================================
// State A anchor target: reply button, repost button, or — once the
// repost menu is open — its "Quote" entry.
// ===================================================================
function quoteMenuItem(): HTMLElement | null {
  // The repost dropdown is a [role="menu"] with "Repost" + "Quote" items.
  // Prefer a stable testid if x.com exposes one; else scan menu items for
  // the "Quote" label. (Iteration point.)
  const byTestId = document.querySelector<HTMLElement>(
    '[role="menu"] [data-testid="quote"]',
  );
  if (byTestId) return byTestId;
  const menu = document.querySelector('[role="menu"]');
  if (!menu) return null;
  for (const item of menu.querySelectorAll<HTMLElement>('[role="menuitem"]')) {
    const t = item.textContent?.trim().toLowerCase() ?? "";
    if (t.includes("quote")) return item;
  }
  return null;
}

function stateAAnchor(): HTMLElement | null {
  if (!current) return null;
  if (current.kind === "quote") {
    return quoteMenuItem() ?? document.querySelector<HTMLElement>(REPOST_BUTTON);
  }
  return document.querySelector<HTMLElement>(REPLY_BUTTON);
}

// ===================================================================
// Positioning helpers
// ===================================================================
function anchorRightOf(w: HelperWidget, target: HTMLElement | null) {
  const el = w.element;
  if (!target) {
    // Unrecognized state — park top-center so the operator still sees it.
    el.style.display = "";
    el.style.top = "16px";
    el.style.left = "50%";
    el.style.transform = "translateX(-50%)";
    return;
  }
  const rect = target.getBoundingClientRect();
  el.style.display = "";
  el.style.top = `${rect.top + rect.height / 2}px`;
  el.style.left = `${rect.right + 8}px`;
  el.style.transform = "translateY(-50%)";
}

function anchorLeftOf(w: HelperWidget, target: HTMLElement | null) {
  const el = w.element;
  if (!target) {
    // Unrecognized state — park top-center so the operator still sees it.
    el.style.display = "";
    el.style.top = "16px";
    el.style.left = "50%";
    el.style.transform = "translateX(-50%)";
    return;
  }
  const rect = target.getBoundingClientRect();
  el.style.display = "";
  el.style.top = `${rect.top + rect.height / 2}px`;
  // Right edge of the badge sits 8px left of the target's left edge.
  el.style.left = `${rect.left - 8}px`;
  el.style.transform = "translate(-100%, -50%)";
}

function hide(w: HelperWidget | null) {
  if (w) w.element.style.display = "none";
}

// ===================================================================
// Mount / teardown
// ===================================================================
function mount() {
  rootEl = document.createElement("div");
  rootEl.id = "__psyops_deliver_helper";
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

  const verb = current?.kind === "quote" ? "quote" : "reply";
  // State A badge anchors to the RIGHT of the reply/quote button (arrow
  // points left at it). State B badges anchor to the LEFT of the composer
  // body + Post button (arrow points right at them).
  clickAction = createHelperWidget({ text: "Click here", arrow: "left" });
  copyBody = createHelperWidget({
    text: "Paste here",
    copyText: current?.content ?? "",
    copyButtonLabel: `Copy ${verb}`,
    arrow: "right",
  });
  clickPost = createHelperWidget({ text: "Click here", arrow: "right" });
  for (const w of [clickAction, copyBody, clickPost]) {
    w.element.style.display = "none";
    shadow.appendChild(w.element);
  }

  // Always-visible bottom-center button. Before send it's "Skip this one"
  // (the only skip path — there is no timeout); after a detected send it
  // becomes "Continue" (operator confirms the post landed, then advances).
  bottomBtn = document.createElement("button");
  bottomBtn.type = "button";
  bottomBtn.textContent = "Skip this one";
  Object.assign(bottomBtn.style, {
    position: "fixed",
    bottom: "16px",
    left: "50%",
    transform: "translateX(-50%)",
    padding: "8px 16px",
    background: "rgba(20, 25, 35, 0.95)",
    color: "#fff",
    font: '13px/1.2 system-ui, -apple-system, "Segoe UI", sans-serif',
    border: "1.5px solid rgba(255, 130, 130, 0.6)",
    borderRadius: "8px",
    boxShadow: "0 4px 12px rgba(0, 0, 0, 0.35)",
    cursor: "pointer",
    pointerEvents: "auto",
  } satisfies Partial<CSSStyleDeclaration>);
  // One handler, dispatched on `sent`: skip before send, advance after.
  bottomBtn.addEventListener("click", () => report(sent ? "done" : "skip"));
  shadow.appendChild(bottomBtn);

  document.body.appendChild(rootEl);
}

function teardown() {
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
  if (urlUnsub) {
    urlUnsub();
    urlUnsub = null;
  }
  document.removeEventListener("click", onPostClick, true);
  rootEl?.remove();
  rootEl = null;
  clickAction = copyBody = clickPost = null;
  bottomBtn = null;
  current = null;
}

function report(status: "done" | "skip") {
  if (reported) return;
  reported = true;
  const item = current;
  teardown();
  if (!item) return;
  invoke("deliver_report", {
    tweet_id: item.tweet_id,
    kind: item.kind,
    status,
  }).catch(() => {});
}

// ===================================================================
// Signal 1+2: Post clicked while green → arm.
// ===================================================================
function onPostClick(e: MouseEvent) {
  if (!current || reported || sent || !onCompose) return;
  const target = e.target as Element | null;
  if (target && target.closest && target.closest(POST_BUTTON)) {
    // Recompute green LIVE at click time — the click only counts if the
    // body matches at this exact instant.
    if (bodyMatches()) armed = true;
  }
}

// ===================================================================
// Signal 3: URL transitions to / from /compose/post.
// ===================================================================
function onUrl(url: string) {
  if (!current || reported || sent) return;
  const nowCompose = isComposeUrl(url);
  if (onCompose && !nowCompose) {
    // Left the composer. The 3 checks are met iff a green Post click armed
    // us. Don't report yet — mark sent and let the operator confirm via
    // "Continue" (X's "Post sent!" toast can lag).
    if (armed) {
      markSent();
      return;
    }
    // Recover: never clicked, clicked-not-green, or cancelled-while-green.
    armed = false;
  }
  if (!onCompose && nowCompose) {
    // Entered a fresh composer.
    armed = false;
  }
  onCompose = nowCompose;
}

/// Post detected as sent. Swap the bottom button Skip -> Continue and
/// freeze the UI in a confirmed (green) state; the operator advances when
/// they've verified the post landed.
function markSent() {
  sent = true;
  if (bottomBtn) {
    bottomBtn.textContent = "Continue";
    bottomBtn.style.borderColor = "rgba(120, 220, 150, 0.6)";
  }
}

// ===================================================================
// Render tick
// ===================================================================
function tick() {
  if (!current || reported) return;

  if (sent) {
    // ---- Sent: awaiting the operator's "Continue" ----
    // Hide the anchored badges (post is in; don't re-prompt State A on the
    // status page we landed back on). The bottom button now reads
    // "Continue".
    hide(clickAction);
    hide(copyBody);
    hide(clickPost);
  } else if (!onCompose) {
    // ---- State A: tweet page ----
    hide(copyBody);
    hide(clickPost);
    if (clickAction) {
      clickAction.setState("incomplete");
      anchorRightOf(clickAction, stateAAnchor());
    }
  } else {
    // ---- State B: composer ----
    hide(clickAction);
    const matched = bodyMatches();
    // A green→non-green edit after an armed click means the post did not
    // go through; disarm so a later cancel can't be mistaken for a post.
    if (armed && !matched) armed = false;

    if (copyBody) {
      copyBody.setState(matched ? "complete" : "incomplete");
      anchorLeftOf(copyBody, document.querySelector<HTMLElement>(COMPOSER));
    }
    if (clickPost) {
      // RED (blocked) until the body matches, then GREEN (complete).
      clickPost.setState(matched ? "complete" : "blocked");
      anchorLeftOf(
        clickPost,
        document.querySelector<HTMLElement>(POST_BUTTON),
      );
    }
  }

  rafId = requestAnimationFrame(tick);
}

// ===================================================================
// Entry: start one item's flow.
// ===================================================================
function deliver(item: DeliverItem) {
  // Clear any half-finished previous flow first.
  if (rootEl || urlUnsub || rafId !== null) {
    teardown();
  }
  current = item;
  reported = false;
  onCompose = false;
  armed = false;
  sent = false;

  mount();
  document.addEventListener("click", onPostClick, true);
  // subscribeUrl fires immediately with the current URL, seeding onCompose.
  urlUnsub = subscribeUrl(onUrl);
  rafId = requestAnimationFrame(tick);
}

/**
 * Suppress x.com's `beforeunload` "Leave site? Changes you made may not be
 * saved." dialog. The driver navigates between target tweets and force-
 * closes the browser at the end of a batch; x.com registers a
 * `beforeunload` handler (compose-draft guard) that otherwise pops a native
 * confirm on every such navigation/close, blocking the flow.
 *
 * We register in the CAPTURE phase. The overlay bundle runs at
 * `on_load_start` — before any page script — so this listener is first in
 * the capture order on `window`; `stopImmediatePropagation` then prevents
 * x.com's own `beforeunload` listeners from running, so none of them set
 * `returnValue` and the browser shows no dialog. (Delivery is a controlled
 * tool — there is no operator "work" to protect; the post is already
 * submitted before any navigation.)
 */
function suppressBeforeUnload(): void {
  window.addEventListener(
    "beforeunload",
    (e) => {
      // Stop x.com's own beforeunload listeners from running — we're first
      // in the capture order (overlay loads at on_load_start), so they
      // never get to set `returnValue` and no dialog is shown.
      //
      // Do NOT touch `e.returnValue` here: it's a DOMString attribute, so
      // assigning anything (even `undefined`, which coerces to the string
      // "undefined") is a non-empty value that itself triggers the prompt.
      // Leaving it at its default "" is what keeps the dialog away.
      e.stopImmediatePropagation();
    },
    true,
  );
}

/**
 * Install the delivery entrypoint. Registers `window.__psyops_deliver`,
 * which the Rust driver calls (via `execute_overlay_js`) once per item.
 */
export function installDeliverHelpers(): void {
  suppressBeforeUnload();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).__psyops_deliver = (item: DeliverItem) => {
    try {
      deliver(item);
    } catch {
      // Fall back to skip so the Rust driver never hangs on this item.
      report("skip");
    }
  };
}
