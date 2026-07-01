// Twitch developer-console app-setup wizard (Mode::TwitchApp).
//
// Two jobs, mirroring the Discord login wizard:
//   1. Auto-scrape the app's credential values from the dev console
//      (read-only) and post each to Rust as `twitch_capture { field, value }`.
//      Rust accumulates them in the in-memory form (no DB write); once BOTH
//      `client_id` and `client_secret` are present it emits the success item
//      and closes. The secret is only revealed once, right after the operator
//      clicks "New Secret", so it's scraped on whichever tick it appears.
//   2. "Click here" pointers that guide the navigation steps (create the app /
//      generate a new secret) plus a persistent instruction badge telling the
//      operator to register the OAuth Redirect URL EXACTLY as the fixed
//      loopback callback the Rust authorize flow binds. A Copy button puts the
//      exact URL on the clipboard.
//
// TOS posture matches the X/Discord helpers: read-only DOM observation +
// render under our own shadow root. No `.value=`, `.click()`,
// `.dispatchEvent`, or fetch.

import {
  HELPER_CSS,
  clampHelperIntoViewport,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { invoke } from "./ipc";

const HELPER_TEXT = "Click here";

// The OAuth redirect URL the Rust authorize flow's callback server binds
// (twitch_authorize.rs `DEFAULT_REDIRECT_URI` / `CALLBACK_PORT`). Twitch
// requires an EXACT match against what's registered here, so the operator must
// paste this verbatim into the app's "OAuth Redirect URLs" field.
const REDIRECT_URL = "http://localhost:17563/psychological-operations/callback";

// --- value scraping (read-only) ------------------------------------------
// The Twitch client_id is a 30-char lowercase-alphanumeric string. The
// client_secret is also 30 chars alphanumeric but is generated with mixed
// case, so an uppercase letter is what separates it from the id on the page.
// These selectors are heuristic and will need real-world tuning against the
// live console (like Discord's did).
const isLowerAlnum = (c: string) =>
  (c >= "a" && c <= "z") || (c >= "0" && c <= "9");
const isAlnum = (c: string) =>
  (c >= "A" && c <= "Z") || isLowerAlnum(c);

/** The single 30-char all-lowercase-alphanumeric leaf (the client_id). */
function uniqueClientId(): string | null {
  return uniqueLeaf(
    (t) => t.length === 30 && [...t].every(isLowerAlnum),
  );
}

/** The single 30-char mixed-case alphanumeric leaf that is NOT the client_id
 *  (the freshly-revealed client_secret). Requiring an uppercase char keeps a
 *  lowercase-only client_id from ever matching as the secret. */
function uniqueClientSecret(clientId: string | null): string | null {
  return uniqueLeaf(
    (t) =>
      t.length === 30 &&
      t !== clientId &&
      [...t].every(isAlnum) &&
      [...t].some((c) => c >= "A" && c <= "Z"),
  );
}

function uniqueLeaf(pred: (t: string) => boolean): string | null {
  const found = new Set<string>();
  for (const e of document.querySelectorAll("*")) {
    if (e.children.length) continue;
    const t = (e.textContent ?? "").trim();
    if (pred(t)) found.add(t);
  }
  return found.size === 1 ? [...found][0] : null;
}

// --- capture state -------------------------------------------------------
type FieldName = "client_id" | "client_secret";

// What THIS overlay has captured, held locally so the pointer logic reads it
// immediately (Rust only pushes state back on a *change*, so the overlay's
// copy could otherwise lag a successful capture).
const captured: Partial<Record<FieldName, string>> = {};
const reported = new Set<FieldName>();

function report(field: FieldName, v: string): void {
  if (captured[field] || reported.has(field)) return;
  reported.add(field);
  captured[field] = v; // optimistic — the pointer logic reads this at once
  invoke("twitch_capture", { field, value: v }).catch(() => {
    reported.delete(field); // let a later tick retry
    delete captured[field];
  });
}

function scrapeAndReport(): void {
  const clientId = captured.client_id ?? uniqueClientId();
  if (clientId) report("client_id", clientId);
  // The secret only shows up (once) after "New Secret". Scan until captured.
  if (!captured.client_secret) {
    const secret = uniqueClientSecret(clientId);
    if (secret) report("client_secret", secret);
  }
}

// --- navigation pointers -------------------------------------------------
function byExactText(text: string): () => HTMLElement | null {
  const want = text.toLowerCase();
  return () => {
    for (const e of document.querySelectorAll<HTMLElement>(
      "a,button,[role=button]",
    )) {
      if ((e.innerText || "").trim().toLowerCase() === want) return e;
    }
    return null;
  };
}

/** The "OAuth Redirect URLs" input/label to anchor the register-URL
 *  instruction near, if present. Matched loosely by nearby text — the field
 *  itself and its label churn, so we look for any leaf mentioning "redirect".
 *  Falls back to `null` (badge pins to a corner). */
function findRedirectField(): HTMLElement | null {
  for (const e of document.querySelectorAll<HTMLElement>("*")) {
    if (e.children.length) continue;
    if (/redirect/i.test(e.textContent ?? "")) return e;
  }
  return null;
}

type Side = "left" | "right";
type Target = { el: HTMLElement; side: Side };

/** 0-or-1 targets: wrap an optional element with the side to point from. */
function one(el: HTMLElement | null, side: Side): Target[] {
  return el ? [{ el, side }] : [];
}

/** The first navigation step with a target wins. Kept minimal — the console's
 *  exact button labels get tuned against the live page later. */
function navTargets(): Target[] {
  // No app yet / on the apps list: point at Register Your Application.
  const register =
    byExactText("register your application")() ??
    byExactText("+ register your application")() ??
    byExactText("create")();
  if (register && !captured.client_id) return one(register, "left");
  // App exists, secret not yet captured: point at New Secret.
  if (captured.client_id && !captured.client_secret) {
    const newSecret = byExactText("new secret")();
    if (newSecret) return one(newSecret, "left");
  }
  return [];
}

// --- mount / tick --------------------------------------------------------
let rootEl: HTMLDivElement | null = null;
let shadowRoot: ShadowRoot | null = null;
// Pool of "Click here" step badges (grown, never shrunk).
const widgets: HelperWidget[] = [];
// The always-on redirect-URL instruction badge (with a Copy button).
let redirectBadge: HelperWidget | null = null;
let rafId: number | null = null;

function ensureWidgets(n: number): void {
  if (!shadowRoot) return;
  while (widgets.length < n) {
    const w = createHelperWidget({ text: HELPER_TEXT, arrow: "left" });
    w.element.style.display = "none";
    shadowRoot.appendChild(w.element);
    widgets.push(w);
  }
}

/** Position one badge next to its target, then clamp it on-screen. */
function place(w: HelperWidget, t: Target): void {
  const el = w.element;
  el.style.display = "";
  const rect = t.el.getBoundingClientRect();
  el.style.top = `${rect.top + rect.height / 2}px`;
  if (t.side === "left") {
    el.classList.remove("arrow-left");
    el.classList.add("arrow-right");
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translate(-100%, -50%)";
  } else {
    el.classList.remove("arrow-right");
    el.classList.add("arrow-left");
    el.style.left = `${rect.right + 8}px`;
    el.style.transform = "translateY(-50%)";
  }
  clampHelperIntoViewport(el);
}

/** Position the redirect-URL instruction badge. Anchors to the "redirect"
 *  field when one is on the page; otherwise pins to the top-left corner so the
 *  operator always sees the exact URL to register. Hidden once the secret is
 *  captured (setup is essentially done). */
function placeRedirectBadge(): void {
  if (!redirectBadge) return;
  const el = redirectBadge.element;
  if (captured.client_secret) {
    el.style.display = "none";
    return;
  }
  el.style.display = "";
  const anchor = findRedirectField();
  if (anchor) {
    const rect = anchor.getBoundingClientRect();
    el.classList.remove("arrow-right");
    el.classList.add("arrow-left");
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.right + 8}px`;
    el.style.transform = "translateY(-50%)";
  } else {
    el.classList.remove("arrow-left", "arrow-right");
    el.style.top = "12px";
    el.style.left = "12px";
    el.style.transform = "none";
  }
  clampHelperIntoViewport(el);
}

function mount(): void {
  if (rootEl) return;
  rootEl = document.createElement("div");
  rootEl.id = "__psyops_twitch_app_helper";
  Object.assign(rootEl.style, {
    position: "fixed",
    top: "0",
    left: "0",
    width: "0",
    height: "0",
    pointerEvents: "none",
    zIndex: "2147483600",
  } satisfies Partial<CSSStyleDeclaration>);
  shadowRoot = rootEl.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = HELPER_CSS;
  shadowRoot.appendChild(style);
  redirectBadge = createHelperWidget({
    text: `Register this OAuth Redirect URL exactly: ${REDIRECT_URL}`,
    copyText: REDIRECT_URL,
    copyButtonLabel: "Copy URL",
  });
  redirectBadge.element.style.display = "none";
  shadowRoot.appendChild(redirectBadge.element);
  document.body.appendChild(rootEl);
  rafId = requestAnimationFrame(tick);
}

function tick(): void {
  if (!shadowRoot) return;

  // Auto-scrape credentials into the in-memory form every tick.
  scrapeAndReport();

  const targets = navTargets();
  ensureWidgets(targets.length);
  for (let i = 0; i < widgets.length; i++) {
    if (i < targets.length) place(widgets[i], targets[i]);
    else widgets[i].element.style.display = "none";
  }
  placeRedirectBadge();
  rafId = requestAnimationFrame(tick);
}

/** Install the Twitch app-setup wizard. Returns an uninstall closure. */
export function installTwitchAppHelpers(): () => void {
  mount();
  return () => {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    rootEl?.remove();
    rootEl = null;
    shadowRoot = null;
    widgets.length = 0;
    redirectBadge = null;
  };
}
