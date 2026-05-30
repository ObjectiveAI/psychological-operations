// Auth-settings wizard for `console.x.com/accounts/<id>/apps/settings?appId=…`.
//
// Five badges that guide the user through configuring the X
// app's auth settings:
//
//   1. App permissions    → "Read and write and Direct message"
//   2. Type of App        → "Web App"
//   3. Website URL        → github repo (copy button)
//   4. Callback URI       → http://127.0.0.1/callback (copy button)
//   5. Save Changes       → red+✕ until 1–4 green, then gray
//
// Same TOS posture as onboarding-helpers: read-only DOM
// observation + shadow-root rendering + clipboard write on the
// user's Copy click. Forbidden APIs unused (grep .value=,
// .checked=, .click, .dispatchEvent, fetch).

import {
  HELPER_CSS,
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
const CALLBACK_URI_COPY = "http://127.0.0.1/callback";
const REQUIRED_PERMISSIONS = "Read and write and Direct message";
const REQUIRED_APP_TYPE = "Web App";

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

/** Read the text of the "selected" radio inside `container`.
 *  Tries (in order) `aria-checked="true"`,
 *  `data-state="checked"`, `[aria-selected="true"]`,
 *  `input[type=radio]:checked` (label-text fallback). */
function readSelectedRadioText(container: HTMLElement): string | null {
  for (const sel of [
    '[aria-checked="true"]',
    '[data-state="checked"]',
    '[aria-selected="true"]',
  ]) {
    for (const el of container.querySelectorAll<HTMLElement>(sel)) {
      const t = (el.textContent ?? "").trim();
      if (t) return t;
    }
  }
  for (const radio of container.querySelectorAll<HTMLInputElement>(
    'input[type="radio"]',
  )) {
    if (!radio.checked) continue;
    // Try the radio's parent's text or its associated label.
    const lbl = radio.closest("label");
    if (lbl) return (lbl.textContent ?? "").trim();
    const p = radio.parentElement;
    if (p) return (p.textContent ?? "").trim();
  }
  return null;
}

/** Find the input/textarea immediately associated with a label
 *  whose text matches `labelText`. */
function findInputByLabel(labelText: string): HTMLElement | null {
  const heading = findByExactText(labelText);
  if (!heading) return null;
  const container = nearestSectionContainer(heading);
  for (const cand of container.querySelectorAll<HTMLElement>(
    "input, textarea",
  )) {
    if (isVisible(cand)) return cand;
  }
  return null;
}

/** Find the radio-group container associated with a section
 *  whose heading text matches `headingText`. */
function findRadioGroupContainer(headingText: string): HTMLElement | null {
  const heading = findByExactText(headingText);
  if (!heading) return null;
  return nearestSectionContainer(heading);
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
// Step definitions
// =================================================================
type Step = {
  id: "permissions" | "app-type" | "website" | "callback" | "save";
  text: string;
  copyText?: string;
  getTarget(): HTMLElement | null;
  isComplete(el: HTMLElement): boolean;
};

const STEPS: Step[] = [
  {
    id: "permissions",
    text: `Set to "${REQUIRED_PERMISSIONS}"`,
    getTarget: () => findRadioGroupContainer("App permissions"),
    isComplete: (el) => readSelectedRadioText(el) === REQUIRED_PERMISSIONS,
  },
  {
    id: "app-type",
    text: `Set to "${REQUIRED_APP_TYPE}"`,
    getTarget: () => findRadioGroupContainer("Type of App"),
    isComplete: (el) => readSelectedRadioText(el) === REQUIRED_APP_TYPE,
  },
  {
    id: "website",
    text: "Click Copy then paste here",
    copyText: WEBSITE_URL_COPY,
    getTarget: () => findInputByLabel("Website URL"),
    isComplete: (el) =>
      (el as HTMLInputElement).value.trim() === WEBSITE_URL_COPY,
  },
  {
    id: "callback",
    text: "Click Copy then paste here",
    copyText: CALLBACK_URI_COPY,
    getTarget: () => findInputByLabel("Callback URI"),
    isComplete: (el) =>
      (el as HTMLInputElement).value.trim() === CALLBACK_URI_COPY,
  },
  {
    id: "save",
    text: "Click Save",
    getTarget: () => findSaveButton(),
    // Save never auto-completes — clicking it triggers a state
    // change that's observed via the next URL transition.
    isComplete: () => false,
  },
];

// =================================================================
// Lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
const widgets = new Map<string, HelperWidget>();
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;

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
}

function tick() {
  if (!rootEl) return;
  const panelOk = isPanelCondition("configure_auth_settings");

  // First pass: pre-compute completion for each non-Save step so
  // we can gate the Save badge on them.
  const targets = new Map<string, HTMLElement | null>();
  const completes = new Map<string, boolean>();
  for (const step of STEPS) {
    const t = step.getTarget();
    targets.set(step.id, t);
    completes.set(step.id, !!t && step.id !== "save" && step.isComplete(t));
  }
  const nonSaveAllGreen = STEPS.every(
    (s) => s.id === "save" || completes.get(s.id),
  );

  for (const step of STEPS) {
    const widget = widgets.get(step.id);
    if (!widget) continue;
    const el = widget.element;
    const target = targets.get(step.id) ?? null;
    if (!panelOk || !target) {
      el.style.display = "none";
      continue;
    }
    el.style.display = "";

    const rect = target.getBoundingClientRect();
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

    let state: HelperState;
    if (step.id === "save") {
      state = nonSaveAllGreen ? "incomplete" : "blocked";
    } else {
      state = completes.get(step.id) ? "complete" : "incomplete";
    }
    widget.setState(state);
  }

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
