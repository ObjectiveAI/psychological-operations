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

// Report `skip` a touch before the Rust ITEM_TIMEOUT (180s) so a clean
// outcome arrives rather than racing the Rust-side timeout.
const DETECT_TIMEOUT_MS = 170_000;

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

let urlUnsub: (() => void) | null = null;
let rafId: number | null = null;
let detectTimer: ReturnType<typeof setTimeout> | null = null;

// DOM
let rootEl: HTMLDivElement | null = null;
let clickAction: HelperWidget | null = null; // State A "Click here"
let copyBody: HelperWidget | null = null; // State B "Paste here"
let clickPost: HelperWidget | null = null; // State B "Click here" (Post)

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
  clickAction = createHelperWidget({ text: "Click here", arrow: "left" });
  copyBody = createHelperWidget({
    text: "Paste here",
    copyText: current?.content ?? "",
    copyButtonLabel: `Copy ${verb}`,
    arrow: "left",
  });
  clickPost = createHelperWidget({ text: "Click here", arrow: "left" });
  for (const w of [clickAction, copyBody, clickPost]) {
    w.element.style.display = "none";
    shadow.appendChild(w.element);
  }
  document.body.appendChild(rootEl);
}

function teardown() {
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
  if (detectTimer !== null) {
    clearTimeout(detectTimer);
    detectTimer = null;
  }
  if (urlUnsub) {
    urlUnsub();
    urlUnsub = null;
  }
  document.removeEventListener("click", onPostClick, true);
  rootEl?.remove();
  rootEl = null;
  clickAction = copyBody = clickPost = null;
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
  if (!current || reported || !onCompose) return;
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
  if (!current || reported) return;
  const nowCompose = isComposeUrl(url);
  if (onCompose && !nowCompose) {
    // Left the composer. Resolve iff a green Post click armed us.
    if (armed) {
      clickPost?.setState("complete");
      report("done");
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

// ===================================================================
// Render tick
// ===================================================================
function tick() {
  if (!current || reported) return;

  if (!onCompose) {
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
      anchorRightOf(copyBody, document.querySelector<HTMLElement>(COMPOSER));
    }
    if (clickPost) {
      // RED (blocked) until the body matches, then GREEN (complete).
      clickPost.setState(matched ? "complete" : "blocked");
      anchorRightOf(
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
  if (rootEl || urlUnsub || rafId !== null || detectTimer !== null) {
    teardown();
  }
  current = item;
  reported = false;
  onCompose = false;
  armed = false;

  mount();
  document.addEventListener("click", onPostClick, true);
  // subscribeUrl fires immediately with the current URL, seeding onCompose.
  urlUnsub = subscribeUrl(onUrl);
  detectTimer = setTimeout(() => report("skip"), DETECT_TIMEOUT_MS);
  rafId = requestAnimationFrame(tick);
}

/**
 * Install the delivery entrypoint. Registers `window.__psyops_deliver`,
 * which the Rust driver calls (via `execute_overlay_js`) once per item.
 */
export function installDeliverHelpers(): void {
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
