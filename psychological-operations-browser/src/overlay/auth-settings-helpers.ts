// Auth-settings wizard for `console.x.com/accounts/<id>/apps/settings?appId=…`.
//
// Five badges that guide the user through configuring the X
// app's auth settings:
//
//   1. App permissions    → "Read and write and Direct message"
//   2. Type of App        → "Web App"
//   3. Website URL        → github repo (copy button)
//   4. Callback URI       → http://127.0.0.1:14978/psychological-operations/callback (copy button)
//   5. Save Changes       → red+✕ until 1–4 green, then gray
//
// Same TOS posture as onboarding-helpers: read-only DOM
// observation + shadow-root rendering + clipboard write on the
// user's Copy click. Forbidden APIs unused (grep .value=,
// .checked=, .click, .dispatchEvent, fetch).

import { invoke } from "./ipc";
import {
  HELPER_CSS,
  clampHelperIntoViewport,
  createHelperWidget,
  type HelperState,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";
import { isPanelCondition } from "./panel-state";

// =================================================================
// Canonical values
// =================================================================
const WEBSITE_URL_COPY =
  "https://github.com/ObjectiveAI/psychological-operations";
const CALLBACK_URI_COPY = "http://127.0.0.1:14978/psychological-operations/callback";
const REQUIRED_PERMISSIONS = "Read and write and Direct message";
// X actually consolidates "Web App", "Automated App", and "Bot" into
// a single option labeled "Web App, Automated App or Bot" — the
// other choice is "Native App", which we don't want.
const REQUIRED_APP_TYPE = "Web App, Automated App or Bot";

// =================================================================
// URL gate
// =================================================================
function isOnAuthSettings(url: string): boolean {
  try {
    const u = new URL(url);
    return (
      u.host === "console.x.com" &&
      /^\/accounts\/\d+\/apps\/settings\/?$/.test(u.pathname)
    );
  } catch {
    return false;
  }
}

// =================================================================
// DOM helpers
// =================================================================

function isVisible(el: Element | null): el is HTMLElement {
  if (!el) return false;
  const r = (el as HTMLElement).getBoundingClientRect();
  return r.width >= 1 && r.height >= 1;
}

/** Find an element of any of the given tag types whose own
 *  trimmed `textContent` matches `text` exactly. First visible
 *  hit wins. */
function findByExactText(
  text: string,
  selector = "h1, h2, h3, h4, h5, h6, label, legend, span, div, p",
): HTMLElement | null {
  for (const el of document.querySelectorAll<HTMLElement>(selector)) {
    if ((el.textContent ?? "").trim() !== text) continue;
    if (!isVisible(el)) continue;
    return el;
  }
  return null;
}

/** Walk up from `el` and return the smallest ancestor whose
 *  bounding-box contains a form control (input / textarea /
 *  radiogroup) so we have a section container to scan. */
function nearestSectionContainer(el: HTMLElement): HTMLElement {
  let cur: HTMLElement | null = el;
  for (let i = 0; i < 8 && cur; i++) {
    if (
      cur.querySelector(
        'input, textarea, [role="radiogroup"], [role="radio"], button',
      )
    ) {
      return cur;
    }
    cur = cur.parentElement;
  }
  return el;
}

type RadioOption = { el: HTMLElement; selected: boolean };

/** Is the option currently selected?
 *
 *  X's dev portal uses a `<button>` per option with Tailwind
 *  classes that flip between `border-blue-500` (selected) and
 *  `border-gray-700` (unselected). No `aria-checked` /
 *  `data-state` on these — we have to read the className.
 *  Fall back to the Radix conventions for the case where the
 *  page does use ARIA. */
function isOptionSelected(el: HTMLElement): boolean {
  // 1. Tailwind class indicator — X's actual pattern.
  // Exclude `hover:border-blue-500/N` (different token).
  const classes = (el.className || "").split(/\s+/);
  if (classes.includes("border-blue-500")) return true;
  // 2. ARIA / data-state (Radix and similar) on self + 3 ancestors.
  let cur: HTMLElement | null = el;
  for (let i = 0; i < 4 && cur; i++) {
    if (
      cur.getAttribute("aria-checked") === "true" ||
      cur.getAttribute("data-state") === "checked" ||
      cur.getAttribute("aria-selected") === "true"
    ) {
      return true;
    }
    cur = cur.parentElement;
  }
  // 3. Nested native radio input.
  const nestedRadio = el.querySelector<HTMLInputElement>(
    'input[type="radio"]',
  );
  if (nestedRadio?.checked) return true;
  return false;
}

/** Find a radio-style option whose visible text equals `value`.
 *  X renders each option as a `<button>` containing an `<h3>`
 *  title — we match on heading-level elements so wrapping
 *  containers (which also include the description paragraph and
 *  fail the exact-text check) don't trip us up; then climb to
 *  the nearest `<button>` ancestor so the badge anchors to the
 *  whole clickable option, not just its tiny title text. */
function findRadioOption(value: string): RadioOption | null {
  const seen = new Set<HTMLElement>();
  const cands: HTMLElement[] = [];
  for (const sel of [
    "h1, h2, h3, h4, h5, h6",
    '[role="radio"]',
    "label",
    "button",
    "div, span",
  ]) {
    for (const el of document.querySelectorAll<HTMLElement>(sel)) {
      if ((el.textContent ?? "").trim() !== value) continue;
      if (!isVisible(el)) continue;
      const btn = el.closest("button") as HTMLElement | null;
      const anchor = btn ?? el;
      if (seen.has(anchor)) continue;
      seen.add(anchor);
      cands.push(anchor);
    }
  }
  if (cands.length === 0) return null;
  cands.sort((a, b) => {
    const ra = a.getBoundingClientRect();
    const rb = b.getBoundingClientRect();
    return ra.width * ra.height - rb.width * rb.height;
  });
  const pick = cands[0]!;
  return { el: pick, selected: isOptionSelected(pick) };
}

/** Find a Website-URL / Callback-URI-style input. Three
 *  strategies, first hit wins:
 *    1. `<label for="…">` association (htmlFor or nesting).
 *    2. `<input>` / `<textarea>` whose placeholder /
 *       aria-label / name / id contains every word of the
 *       label text (≥3 chars).
 *    3. Heading-text → walk up to nearest section container →
 *       first visible input inside (the original strategy). */
function findFieldInput(labelText: string): HTMLElement | null {
  // (1) <label> association. Match by `startsWith` so a label like
  // "Callback URI / Redirect URL(required)" (text + nested
  // (required) span) still matches a query for "Callback URI".
  for (const lbl of document.querySelectorAll<HTMLLabelElement>("label")) {
    const lblText = (lbl.textContent ?? "").trim();
    if (!lblText.startsWith(labelText)) continue;
    if (lbl.htmlFor) {
      const el = document.getElementById(lbl.htmlFor);
      if (el && isVisible(el)) return el as HTMLElement;
    }
    const inside = lbl.querySelector<HTMLElement>("input, textarea");
    if (inside && isVisible(inside)) return inside;
  }

  // (2) attribute-needle match
  const needles = labelText
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((s) => s.length >= 3);
  if (needles.length > 0) {
    for (const el of document.querySelectorAll<HTMLElement>(
      "input, textarea",
    )) {
      if (!isVisible(el)) continue;
      const hay = [
        el.getAttribute("placeholder") ?? "",
        el.getAttribute("aria-label") ?? "",
        el.getAttribute("name") ?? "",
        el.getAttribute("id") ?? "",
      ]
        .join(" ")
        .toLowerCase();
      if (needles.every((n) => hay.includes(n))) return el;
    }
  }

  // (3) heading-then-container fallback
  const heading = findByExactText(labelText);
  if (heading) {
    const container = nearestSectionContainer(heading);
    for (const cand of container.querySelectorAll<HTMLElement>(
      "input, textarea",
    )) {
      if (isVisible(cand)) return cand;
    }
  }

  return null;
}

/** Save Changes / Save button. */
function findSaveButton(): HTMLButtonElement | null {
  for (const b of document.querySelectorAll<HTMLButtonElement>("button")) {
    const t = (b.textContent ?? "").trim();
    if (t === "Save" || t === "Save Changes" || /^Save\b/.test(t)) {
      if (isVisible(b)) return b;
    }
  }
  return null;
}

// =================================================================
// OAuth 2.0 popup — fires after Save Changes, surfaces Client ID
// + Client Secret. Structurally identical to the post-create
// dialog detection.
// =================================================================

/** Find the OAuth 2.0 popup (the one displayed after Save).
 *  Identified by `[role="dialog"]` whose visible text contains
 *  both "Client ID" and "Client Secret" (lowercased). Other
 *  dialogs that might fire on this page won't carry both
 *  labels at once. */
function findOAuthPopup(): HTMLElement | null {
  for (const d of document.querySelectorAll<HTMLElement>('[role="dialog"]')) {
    const text = (d.textContent ?? "").toLowerCase();
    if (text.includes("client id") && text.includes("client secret")) {
      return d;
    }
  }
  return null;
}

/** Close button inside the OAuth popup. Same heuristic the
 *  post-create-dialog helper uses — close / done / got it / etc. */
function findOAuthPopupClose(dialog: HTMLElement): HTMLButtonElement | null {
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
// Step definitions
// =================================================================
/** Per-step contract: find the target element + decide
 *  whether the step is currently satisfied. `save` steps gate
 *  the user's click and never "complete" on their own. */
type Step = {
  id: "permissions" | "app-type" | "website" | "callback" | "save";
  text: string;
  copyText?: string;
  resolve(): { el: HTMLElement; selected?: boolean } | null;
  /** Override for steps whose completion isn't derivable from
   *  the resolved element directly (radio-option selected state
   *  is already carried in the resolve return). */
  isComplete?(el: HTMLElement): boolean;
};

const STEPS: Step[] = [
  {
    id: "permissions",
    text: "Click here",
    resolve: () => findRadioOption(REQUIRED_PERMISSIONS),
  },
  {
    id: "app-type",
    text: "Click here",
    resolve: () => findRadioOption(REQUIRED_APP_TYPE),
  },
  {
    id: "website",
    text: "Click Copy then paste here",
    copyText: WEBSITE_URL_COPY,
    resolve: () => {
      const el = findFieldInput("Website URL");
      return el ? { el } : null;
    },
    isComplete: (el) =>
      (el as HTMLInputElement).value.trim() === WEBSITE_URL_COPY,
  },
  {
    id: "callback",
    text: "Click Copy then paste here",
    copyText: CALLBACK_URI_COPY,
    resolve: () => {
      const el = findFieldInput("Callback URI");
      return el ? { el } : null;
    },
    isComplete: (el) =>
      (el as HTMLInputElement).value.trim() === CALLBACK_URI_COPY,
  },
  {
    id: "save",
    text: "Click Save",
    resolve: () => {
      const el = findSaveButton();
      return el ? { el } : null;
    },
    isComplete: () => false,
  },
];

// =================================================================
// Lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
const widgets = new Map<string, HelperWidget>();
let closeWidget: HelperWidget | null = null;
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;

// =================================================================
// OAuth-popup capture throttle. Same shape as
// post-create-dialog-helpers: don't double-fire while a request is
// in flight, and resend at most every RESEND_INTERVAL_MS.
// =================================================================
const RESEND_INTERVAL_MS = 2_000;
let popupLastSendAt = 0;
let popupInFlight = false;
let popupCaptured = false;

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_auth_settings_helpers";
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

  for (const step of STEPS) {
    const widget = createHelperWidget({
      text: step.text,
      copyText: step.copyText,
      arrow: "right",
    });
    widget.element.dataset.step = step.id;
    widget.element.style.display = "none";
    widgets.set(step.id, widget);
    shadow.appendChild(widget.element);
  }

  // OAuth-popup close badge — created up front, hidden until the
  // popup appears.
  closeWidget = createHelperWidget({ text: "Click close", arrow: "right" });
  closeWidget.element.dataset.step = "oauth-close";
  closeWidget.element.style.display = "none";
  shadow.appendChild(closeWidget.element);

  popupLastSendAt = 0;
  popupInFlight = false;
  popupCaptured = false;

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
  widgets.clear();
  closeWidget = null;
  popupLastSendAt = 0;
  popupInFlight = false;
  popupCaptured = false;
}

/** Position the close badge against the popup's Close button
 *  (or top-right of the popup as a fallback), throttle the
 *  HTML send to Rust, and reflect the captured/in-flight
 *  state on the badge. */
function handleOAuthPopup(popup: HTMLElement) {
  if (!closeWidget) return;
  const el = closeWidget.element;
  el.style.display = "";

  const close = findOAuthPopupClose(popup);
  if (close) {
    const rect = close.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translateX(-100%) translateY(-50%)";
  } else {
    const rect = popup.getBoundingClientRect();
    el.style.top = `${rect.top + 12}px`;
    el.style.left = `${rect.right - 12}px`;
    el.style.transform = "translateX(-100%)";
  }

  const VIEWPORT_MARGIN = 12;
  const MIN_WIDTH = 120;
  const MAX_WIDTH = 300;
  const leftEdgeRoom = close
    ? close.getBoundingClientRect().left - 8 - VIEWPORT_MARGIN
    : MAX_WIDTH;
  el.style.maxWidth = `${Math.max(
    MIN_WIDTH,
    Math.min(MAX_WIDTH, leftEdgeRoom),
  )}px`;

  // Throttled HTML send.
  const now = performance.now();
  if (
    !popupCaptured &&
    !popupInFlight &&
    now - popupLastSendAt >= RESEND_INTERVAL_MS
  ) {
    popupLastSendAt = now;
    popupInFlight = true;
    const html = document.documentElement.outerHTML;
    invoke<number>("process_oauth_popup_html", { html })
      .then((stored) => {
        if (stored >= 2) popupCaptured = true;
      })
      .catch(() => {
        // Most common: "no user_id yet". Quietly retry next tick.
      })
      .finally(() => {
        popupInFlight = false;
      });
  }

  closeWidget.setState(popupCaptured ? "complete" : "blocked");
}

function tick() {
  if (!rootEl) return;
  const panelOk = isPanelCondition("configure_auth_settings");

  // OAuth-popup branch: when the popup is open, we take over —
  // hide every wizard badge (they reference fields obscured by
  // the popup) and pin the close badge to the popup's Close
  // button instead. Falls through to the wizard logic only when
  // the popup isn't present.
  const popup = panelOk ? findOAuthPopup() : null;
  if (popup && closeWidget) {
    for (const w of widgets.values()) w.element.style.display = "none";
    handleOAuthPopup(popup);
    rafId = requestAnimationFrame(tick);
    return;
  }
  if (closeWidget) {
    closeWidget.element.style.display = "none";
  }
  // Popup just closed (or never opened) — reset throttle so the
  // next open re-fires the capture immediately.
  popupLastSendAt = 0;
  popupInFlight = false;
  popupCaptured = false;

  // Resolve every step's target + completion in one pass so we
  // can gate Save on the others.
  type Resolved = { el: HTMLElement | null; complete: boolean };
  const resolved = new Map<string, Resolved>();
  for (const step of STEPS) {
    const r = step.resolve();
    let complete = false;
    if (r) {
      if (step.id === "permissions" || step.id === "app-type") {
        complete = !!r.selected;
      } else if (step.isComplete) {
        complete = step.isComplete(r.el);
      }
    }
    resolved.set(step.id, { el: r?.el ?? null, complete });
  }
  const nonSaveAllGreen = STEPS.every(
    (s) => s.id === "save" || resolved.get(s.id)?.complete,
  );

  STEPS.forEach((step, stepIndex) => {
    const widget = widgets.get(step.id);
    if (!widget) return;
    const el = widget.element;
    const r = resolved.get(step.id)!;
    if (!panelOk) {
      el.style.display = "none";
      return;
    }

    if (!r.el) {
      // Diagnostic fallback so we know which finder missed.
      // Stack in the top-right column, one badge per step.
      el.style.display = "";
      widget.setState("blocked");
      widget.setText(`${step.id} target not found`);
      el.style.top = `${12 + stepIndex * 44}px`;
      el.style.left = `${window.innerWidth - 12}px`;
      el.style.transform = "translateX(-100%)";
      el.style.maxWidth = "300px";
      return;
    }

    el.style.display = "";
    widget.setText(step.text);

    const rect = r.el.getBoundingClientRect();
    const GAP = 8;
    const VIEWPORT_MARGIN = 12;
    const MIN_WIDTH = 140;
    const MAX_WIDTH = 320;
    const available = rect.left - GAP - VIEWPORT_MARGIN;
    el.style.maxWidth = `${Math.max(
      MIN_WIDTH,
      Math.min(MAX_WIDTH, available),
    )}px`;
    if (rect.height > 60) {
      el.style.top = `${rect.top + 8}px`;
      el.style.transform = "translateX(-100%)";
    } else {
      el.style.top = `${rect.top + rect.height / 2}px`;
      el.style.transform = "translateX(-100%) translateY(-50%)";
    }
    el.style.left = `${rect.left - GAP}px`;
    clampHelperIntoViewport(el);

    let state: HelperState;
    if (step.id === "save") {
      // When all four prior badges are green, Save should show
      // green + ✓ ("ready to click"), not gray-incomplete. Until
      // then, blocked (red+✕) to make the gate obvious.
      state = nonSaveAllGreen ? "complete" : "blocked";
    } else if (step.id === "permissions" || step.id === "app-type") {
      // Clicker pointers: red+✕ until correctly selected, then
      // green+✓.
      state = r.complete ? "complete" : "blocked";
    } else {
      state = r.complete ? "complete" : "incomplete";
    }
    widget.setState(state);
  });

  rafId = requestAnimationFrame(tick);
}

// =================================================================
// Public entry point
// =================================================================
export function installAuthSettingsHelpers(): () => void {
  urlUnsubscribe = subscribeUrl((url) => {
    if (isOnAuthSettings(url)) {
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
