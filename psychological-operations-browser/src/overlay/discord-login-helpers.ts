// Discord developer-portal login wizard (Mode::DiscordLogin).
//
// Step 1 — sign-in: detect the signed-out portal (the "Log In" button is
// present), report sign-in state to Rust via the `discord_signed_in` scheme
// call, and render a single "Click here" pointer anchored to the right of
// the Log In button. The pointer is gated on the panel condition
// `sign_in_to_discord` so it stays in lockstep with the header — mirrors
// apps-tab-helper. Later wizard steps (create app, add bot, reveal token)
// hang off this same module.
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

/**
 * The portal's "Log In" button. Discord's class names are hashed and churn
 * across builds, so match on the visible text rather than a selector.
 */
function findLoginButton(): HTMLElement | null {
  for (const e of document.querySelectorAll<HTMLElement>(
    "a,button,[role=button]",
  )) {
    if ((e.innerText || "").trim().toLowerCase() === "log in") return e;
  }
  return null;
}

let rootEl: HTMLDivElement | null = null;
let widget: HelperWidget | null = null;
let rafId: number | null = null;
let lastSignedIn: boolean | null = null;

/** Report sign-in state to Rust on change (drives the panel header). */
function reportSignedIn(signedIn: boolean): void {
  if (lastSignedIn === signedIn) return;
  lastSignedIn = signedIn;
  invoke("discord_signed_in", { signed_in: signedIn }).catch(() => {});
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
  const btn = findLoginButton();

  // Log In button present ⇒ signed out. Report so the panel header flips.
  reportSignedIn(!btn);

  const el = widget.element;
  // Show only when the button is present AND the panel header is asking the
  // user to log in (mirrored from Rust) — keeps pointer + header in sync.
  const show = !!btn && isPanelCondition("sign_in_to_discord");
  if (!show || !btn) {
    el.style.display = "none";
  } else {
    el.style.display = "";
    const rect = btn.getBoundingClientRect();
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
