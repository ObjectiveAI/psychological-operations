// Discord developer-portal login wizard (Mode::DiscordLogin).
//
// A small DOM state machine: each tick it finds the active wizard step by
// probing the page (in priority order), reports the step name to Rust via
// the `discord_step` scheme call (which drives the panel header), and parks
// a single "Click here" pointer on the active step's target — gated on the
// step's panel condition so pointer + header stay in lockstep (mirrors the
// X helpers' apps-tab pattern).
//
// Adding a step = one entry in STEPS (+ a Rust `derive` arm mapping its
// name to a PanelCondition). Steps so far: log_in, skip. Create-app / add-
// bot / reveal-token hang off here next.
//
// TOS posture matches the X helpers: read-only DOM observation + rendering
// under our own shadow root. No `.value=`, `.click()`, `.dispatchEvent`, or
// fetch to discord.com.

import {
  HELPER_CSS,
  createHelperWidget,
  type HelperWidget,
} from "./helper-widget";
import { isPanelCondition } from "./panel-state";
import { invoke } from "./ipc";

const HELPER_TEXT = "Click here";

/** A wizard step: how to find its target on the page + the panel condition
 *  that gates its pointer. Discord's class names are hashed and churn, so
 *  steps match on visible text rather than selectors. */
type Step = { name: string; condition: string; find: () => HTMLElement | null };

/** Find a clickable element whose visible text is exactly `text`. */
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

// Checked in order; the first step whose target is present is active.
const STEPS: Step[] = [
  { name: "log_in", condition: "sign_in_to_discord", find: byExactText("log in") },
  { name: "skip", condition: "discord_skip", find: byExactText("skip") },
];

let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;
let lastStep: string | null = null;

/** Report the active step to Rust on change (drives the panel header). */
function reportStep(step: string | null): void {
  if (lastStep === step) return;
  lastStep = step;
  invoke("discord_step", { step }).catch(() => {});
}

function activeStep(): { step: Step; el: HTMLElement } | null {
  for (const step of STEPS) {
    const el = step.find();
    if (el) return { step, el };
  }
  return null;
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
  const active = activeStep();

  // Report the current step so Rust flips the panel header.
  reportStep(active?.step.name ?? null);

  const el = widget.element;
  // Show only when there's an active step AND the panel header is on that
  // step's condition (mirrored from Rust) — keeps pointer + header in sync.
  const show = !!active && isPanelCondition(active.step.condition);
  if (!show || !active) {
    el.style.display = "none";
  } else {
    el.style.display = "";
    const rect = active.el.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.right + 8}px`;
    el.style.transform = "translateY(-50%)";
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
