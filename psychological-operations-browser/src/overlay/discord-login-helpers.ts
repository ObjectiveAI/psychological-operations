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
  if (!appId || !publicKey) return;
  report("application_id", appId);
  report("public_key", publicKey);
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
function intentToggles(): { sw: HTMLInputElement; on: boolean }[] {
  const switches = [
    ...document.querySelectorAll<HTMLInputElement>("input[role=switch]"),
  ];
  const used = new Set<HTMLInputElement>();
  const out: { sw: HTMLInputElement; on: boolean }[] = [];
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
      out.push({ sw, on: sw.checked });
    }
  }
  return out;
}

/** Every privileged-intent toggle still switched off. */
function intentsOff(): HTMLElement[] {
  return intentToggles()
    .filter((t) => !t.on)
    .map((t) => t.sw);
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
    // On the Bot page: a separate pointer at every privileged intent still
    // off, plus the Save Changes bar once it appears — all shown at once.
    find: (): Target[] => {
      if (!onBotPage() || !pending("bot_token")) return [];
      const targets = intentsOff().map((el): Target => ({ el, side: "left" }));
      return targets.concat(one(saveChanges(), "left"));
    },
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
