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

type Step = { name: string; side: "left" | "right"; find: () => HTMLElement | null };

const STEPS: Step[] = [
  { name: "log_in", side: "right", find: byExactText("log in") },
  { name: "skip", side: "right", find: byExactText("skip") },
  { name: "create_app", side: "left", find: byExactText("new application") },
  {
    name: "open_bot",
    side: "right",
    // Once app id + public key are captured and the token isn't yet.
    find: () =>
      value("application_id") && value("public_key") && pending("bot_token")
        ? findBotLink()
        : null,
  },
];

function activeStep(): { step: Step; el: HTMLElement } | null {
  for (const step of STEPS) {
    const el = step.find();
    if (el) return { step, el };
  }
  return null;
}

// --- mount / tick --------------------------------------------------------
let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;

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
  const shadow = rootEl.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = HELPER_CSS;
  shadow.appendChild(style);
  widget = createHelperWidget({ text: HELPER_TEXT, arrow: "left" });
  shadow.appendChild(widget.element);
  widget.element.style.display = "none";
  document.body.appendChild(rootEl);
  rafId = requestAnimationFrame(tick);
}

function tick(): void {
  if (!widget) return;

  // Auto-scrape credentials into the header form every tick.
  scrapeAndReport();

  const el = widget.element;
  // The create dialog owns its own badges; hide the single pointer.
  const active = isDiscordCreateDialogOpen() ? null : activeStep();
  if (!active) {
    el.style.display = "none";
  } else {
    el.style.display = "";
    const rect = active.el.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    if (active.step.side === "left") {
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
    widget = null;
  };
}
