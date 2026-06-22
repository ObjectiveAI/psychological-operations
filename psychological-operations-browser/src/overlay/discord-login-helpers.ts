// Discord developer-portal login wizard (Mode::DiscordLogin).
//
// Two jobs:
//   1. Auto-scrape the credential values from the portal (read-only) and post
//      each to Rust as `discord_capture { field, value }`. Rust accumulates
//      them in the in-memory header form (no DB write); once ALL fields are
//      present it commits them in one write and closes. Values can arrive in
//      any order across pages. (App ID + Public Key are scraped here on the
//      General Information page; the Bot token scraper lands on the Bot page.)
//   2. A single "Click here" pointer that guides the *navigation* steps
//      (sign in / skip / create app / open the Bot tab) — gated on DOM
//      presence + the header form state.
//
// TOS posture matches the X helpers: read-only DOM observation + render under
// our own shadow root. No `.value=`, `.click()`, `.dispatchEvent`, or fetch.

import {
  HELPER_CSS,
  clampHelperIntoViewport,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { getDiscordAuth } from "./panel-state";
import { invoke } from "./ipc";
import { isDiscordCreateDialogOpen } from "./discord-create-dialog-helpers";

const HELPER_TEXT = "Click here";
const HEX = "0123456789abcdef";

// --- value scraping (read-only) ------------------------------------------
/** The single 17–20 digit leaf on the page (the Application ID), or null. */
function uniqueSnowflake(): string | null {
  return uniqueLeaf(
    (t) => t.length >= 17 && t.length <= 20 && [...t].every((c) => c >= "0" && c <= "9"),
  );
}
/** The single 64-hex leaf on the page (the Public Key), or null. */
function uniquePublicKey(): string | null {
  return uniqueLeaf(
    (t) => t.length === 64 && [...t.toLowerCase()].every((c) => HEX.includes(c)),
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

function isB64Url(s: string): boolean {
  return (
    s.length > 0 &&
    [...s].every(
      (c) =>
        (c >= "A" && c <= "Z") ||
        (c >= "a" && c <= "z") ||
        (c >= "0" && c <= "9") ||
        c === "_" ||
        c === "-",
    )
  );
}

/** A bot token is `<id>.<ts>.<hmac>`: three base64url segments joined by two
 *  dots, ~50-110 chars. The two dots are what separate it from other base64url
 *  blobs on the page (notably the 64-hex public key, which has none). */
function tokenShaped(t: string): boolean {
  if (t.length < 50 || t.length > 110) return false;
  const p = t.split(".");
  return p.length === 3 && p.every((s) => s.length >= 5 && isB64Url(s));
}

/** The revealed bot token. Discord renders it as a bare text node sitting
 *  directly inside a div, as a sibling to the Copy/Reset buttons — it is NOT
 *  wrapped in its own element. So no leaf holds it, and the parent div's
 *  textContent also swallows the button labels. Walk the text nodes instead
 *  and take the single token-shaped value. */
function uniqueBotToken(): string | null {
  const found = new Set<string>();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (node) {
    const t = (node.textContent ?? "").trim();
    if (tokenShaped(t)) found.add(t);
    node = walker.nextNode();
  }
  return found.size === 1 ? [...found][0] : null;
}

// --- header form state ---------------------------------------------------
type FieldName = "application_id" | "public_key" | "bot_token";
function value(field: FieldName): string | undefined {
  return getDiscordAuth()?.[field]?.value;
}
/** Still needs capture (not yet in the header form). */
function pending(field: FieldName): boolean {
  return !value(field);
}

// --- auto-scrape → header ------------------------------------------------
const reported = new Set<FieldName>();

function report(field: FieldName, v: string): void {
  if (value(field) || reported.has(field)) return;
  reported.add(field);
  invoke("discord_capture", { field, value: v }).catch(() => {
    reported.delete(field); // let a later tick retry
  });
}

function scrapeAndReport(): void {
  // Application ID + Public Key both live on the General Information page;
  // require both present so a stray snowflake elsewhere can't misfire.
  const appId = uniqueSnowflake();
  const publicKey = uniquePublicKey();
  if (appId && publicKey) {
    report("application_id", appId);
    report("public_key", publicKey);
  }
  // The bot token is revealed (once) on the Bot page after a reset. Scan only
  // there, and only until captured — the textContent sweep is heavier.
  if (onBotPage() && pending("bot_token")) {
    const token = uniqueBotToken();
    if (token) report("bot_token", token);
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

/** The Bot sidebar link, by its stable href (class names churn). */
function findBotLink(): HTMLElement | null {
  for (const a of document.querySelectorAll<HTMLAnchorElement>("a[href]")) {
    if ((a.getAttribute("href") ?? "").endsWith("/bot")) return a;
  }
  return null;
}

/** True when the content surface is on an app's Bot page (`.../<id>/bot`). */
function onBotPage(): boolean {
  try {
    return new URL(location.href).pathname.endsWith("/bot");
  } catch {
    return false;
  }
}

// --- privileged gateway intents (Bot page) -------------------------------
// The bot needs all three privileged intents on before we reveal the token.
const INTENT_LABELS = [
  "presence intent",
  "server members intent",
  "message content intent",
];

/** Each privileged-intent toggle + on/off state, matched to its label by
 *  DOM order: the row's switch is the first `input[role=switch]` that
 *  *follows* the label. (Pixel proximity fails — the label sits ~64px above
 *  its switch and rows are ~116px tall, so the nearest switch by top is often
 *  the previous row's.) Claimed switches are skipped so two labels can't map
 *  to the same toggle. */
function intentToggles(): { el: HTMLElement; on: boolean }[] {
  const switches = [
    ...document.querySelectorAll<HTMLInputElement>("input[role=switch]"),
  ];
  const used = new Set<HTMLInputElement>();
  const out: { el: HTMLElement; on: boolean }[] = [];
  for (const want of INTENT_LABELS) {
    let label: Element | null = null;
    for (const e of document.querySelectorAll("*")) {
      if (e.children.length) continue;
      if ((e.textContent ?? "").trim().toLowerCase() === want) {
        label = e;
        break;
      }
    }
    if (!label) continue;
    let sw: HTMLInputElement | null = null;
    for (const s of switches) {
      if (used.has(s)) continue;
      if (label.compareDocumentPosition(s) & Node.DOCUMENT_POSITION_FOLLOWING) {
        sw = s;
        break;
      }
    }
    if (sw) {
      used.add(sw);
      // The input[role=switch] is visually hidden (a 1px a11y span); the
      // styled <label> wrapping it is the visible slider — point at that.
      const el = (sw.closest("label") as HTMLElement | null) ?? sw;
      out.push({ el, on: sw.checked });
    }
  }
  return out;
}

/** Every privileged-intent toggle still switched off (the visible slider). */
function intentsOff(): HTMLElement[] {
  return intentToggles()
    .filter((t) => !t.on)
    .map((t) => t.el);
}

/** All three privileged intents are present and on. */
function allIntentsOn(): boolean {
  const t = intentToggles();
  return t.length === INTENT_LABELS.length && t.every((x) => x.on);
}

type Side = "left" | "right";
type Target = { el: HTMLElement; side: Side };
type Step = { name: string; find: () => Target[] };

/** 0-or-1 targets: wrap an optional element with the side to point from. */
function one(el: HTMLElement | null, side: Side): Target[] {
  return el ? [{ el, side }] : [];
}

/** Discord's unsaved-changes bar's Save button, when showing. */
function saveChanges(): HTMLElement | null {
  return byExactText("save changes")();
}

/** Discord sometimes gates the token reset behind an MFA prompt. */
function mfaOpen(): boolean {
  for (const e of document.querySelectorAll("*")) {
    if (e.children.length) continue;
    if (/multi-factor authentication/i.test(e.textContent ?? "")) return true;
  }
  return false;
}

const STEPS: Step[] = [
  { name: "log_in", find: () => one(byExactText("log in")(), "right") },
  { name: "skip", find: () => one(byExactText("skip")(), "right") },
  { name: "create_app", find: () => one(byExactText("new application")(), "left") },
  {
    name: "open_bot",
    // App id + public key captured, token not yet, and not already on /bot.
    find: () =>
      value("application_id") &&
      value("public_key") &&
      pending("bot_token") &&
      !onBotPage()
        ? one(findBotLink(), "right")
        : [],
  },
  {
    name: "bot_setup",
    // On the Bot page: while any privileged intent is still off, a separate
    // pointer at each. Only once all three are on do we point at Save Changes
    // (so we don't nudge them to save a half-done set).
    find: (): Target[] => {
      if (!onBotPage() || !pending("bot_token")) return [];
      const off = intentsOff();
      if (off.length) return off.map((el): Target => ({ el, side: "left" }));
      return one(saveChanges(), "left");
    },
  },
  {
    name: "mfa_submit",
    // Discord sometimes asks for an MFA code before resetting the token.
    // Highest priority among the reset steps: while it's open the Reset Token
    // / Yes-do-it buttons linger in the DOM behind it.
    find: () =>
      pending("bot_token") && mfaOpen() ? one(byExactText("submit")(), "left") : [],
  },
  {
    name: "confirm_reset",
    // The "Reset Bot's Token?" confirmation that pops up after Reset Token.
    // Ahead of reset_token: the Reset Token button stays in the DOM behind
    // the modal, so this must win while the confirmation is open.
    find: () =>
      pending("bot_token") ? one(byExactText("yes, do it!")(), "left") : [],
  },
  {
    name: "reset_token",
    // Only once every intent is on AND saved (no pending changes) do we
    // reveal the token.
    find: () =>
      onBotPage() && pending("bot_token") && allIntentsOn() && !saveChanges()
        ? one(byExactText("reset token")(), "left")
        : [],
  },
];

/** The first step with any targets wins; it may yield several at once. */
function activeTargets(): Target[] {
  if (isDiscordCreateDialogOpen()) return []; // the dialog owns its own badges
  for (const step of STEPS) {
    const t = step.find();
    if (t.length) return t;
  }
  return [];
}

// --- mount / tick --------------------------------------------------------
let rootEl: HTMLDivElement | null = null;
let shadowRoot: ShadowRoot | null = null;
const widgets: HelperWidget[] = [];
let rafId: number | null = null;

/** Grow the widget pool to at least `n` badges (we never shrink it; extras
 *  are just hidden each tick). */
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

function mount(): void {
  if (rootEl) return;
  rootEl = document.createElement("div");
  rootEl.id = "__psyops_discord_login_helper";
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
  document.body.appendChild(rootEl);
  rafId = requestAnimationFrame(tick);
}

function tick(): void {
  if (!shadowRoot) return;

  // Auto-scrape credentials into the header form every tick.
  scrapeAndReport();

  const targets = activeTargets();
  ensureWidgets(targets.length);
  for (let i = 0; i < widgets.length; i++) {
    if (i < targets.length) place(widgets[i], targets[i]);
    else widgets[i].element.style.display = "none";
  }
  rafId = requestAnimationFrame(tick);
}

/** Install the Discord login wizard. Returns an uninstall closure. */
export function installDiscordLoginHelpers(): () => void {
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
  };
}
