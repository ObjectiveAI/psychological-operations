// "Create New Client Application" dialog helpers — three badges
// guiding the user through the three controls in the dialog:
//
//   1. Application Name → "Enter anything"
//   2. Environment      → "Must be Production"
//   3. Dialog Create    → "Click this" (red+✕ until Env=Production)
//
// Mirrors the onboarding-form pattern: anchored badges via the
// shared `helper-widget`, red blocked Create until prereqs met,
// green checkmarks. Activates on /apps URL; per-tick check decides
// whether the dialog is open and runs the badges only then.
//
// TOS posture: read-only DOM observation + render under our own
// shadow root. Forbidden APIs (`.value=`, `.click()`,
// `.dispatchEvent`, fetch) are unused.

import {
  HELPER_CSS,
  createHelperWidget,
  type HelperState,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";
import { isCreateAppDialogOpen } from "./apps-page-helpers";

// =================================================================
// URL gate
// =================================================================
function isOnAppsTab(url: string): boolean {
  try {
    const u = new URL(url);
    return (
      u.host === "console.x.com" &&
      /^\/accounts\/\d+\/apps(\/|$)/.test(u.pathname)
    );
  } catch {
    return false;
  }
}

// =================================================================
// Step definitions
// =================================================================
type DialogStep = {
  id: "name" | "env" | "create";
  text: string;
  /** Find the control to anchor against, scoped to the dialog. */
  getTarget(dialog: HTMLElement): HTMLElement | null;
};

const STEPS: DialogStep[] = [
  {
    id: "name",
    text: "Enter anything",
    // Application Name input. Likely an <input type="text"> with
    // a label association. Try a few patterns:
    getTarget: (dialog) => findApplicationNameInput(dialog),
  },
  {
    id: "env",
    text: "Must be Production",
    getTarget: (dialog) => findEnvironmentSelector(dialog),
  },
  {
    id: "create",
    text: "Click this",
    getTarget: (dialog) => findDialogCreateButton(dialog),
  },
];

// =================================================================
// Selector helpers — best-effort with multiple fallbacks. If X
// changes the dialog's markup the selectors degrade to "no
// target found" → that step's badge silently hides.
// =================================================================
function findApplicationNameInput(dialog: HTMLElement): HTMLInputElement | null {
  // 1. Label-paired: <label for="..."> with text matching Name.
  for (const label of dialog.querySelectorAll<HTMLLabelElement>("label")) {
    const t = label.textContent?.trim() ?? "";
    if (/application\s+name/i.test(t)) {
      const id = label.getAttribute("for");
      if (id) {
        const el = dialog.querySelector<HTMLInputElement>(`#${cssEscape(id)}`);
        if (el && el.tagName === "INPUT") return el;
      }
      // Label has no `for`; try first input descendant.
      const nested = label.querySelector<HTMLInputElement>("input");
      if (nested) return nested;
    }
  }
  // 2. aria-label match on the input itself.
  for (const inp of dialog.querySelectorAll<HTMLInputElement>("input")) {
    const aria = inp.getAttribute("aria-label") ?? "";
    if (/application\s+name/i.test(aria)) return inp;
  }
  // 3. Last resort: first text input in the dialog.
  for (const inp of dialog.querySelectorAll<HTMLInputElement>("input")) {
    const type = (inp.getAttribute("type") ?? "text").toLowerCase();
    if (type === "text" || type === "") return inp;
  }
  return null;
}

function findEnvironmentSelector(dialog: HTMLElement): HTMLElement | null {
  // 1. Label-paired control. Radix selects render as buttons
  //    with role="combobox" or aria-haspopup="listbox".
  for (const label of dialog.querySelectorAll<HTMLLabelElement>("label")) {
    const t = label.textContent?.trim() ?? "";
    if (!/environment/i.test(t)) continue;
    const id = label.getAttribute("for");
    if (id) {
      const el = dialog.querySelector<HTMLElement>(`#${cssEscape(id)}`);
      if (el) return el;
    }
    // Try the next sibling element.
    const sib = label.nextElementSibling as HTMLElement | null;
    if (sib) return sib;
  }
  // 2. role=combobox / select-like button.
  const combo = dialog.querySelector<HTMLElement>(
    '[role="combobox"], select, [aria-haspopup="listbox"]',
  );
  if (combo) return combo;
  return null;
}

function findDialogCreateButton(dialog: HTMLElement): HTMLButtonElement | null {
  // Find buttons whose text starts with "Create" — likely
  // "Create" or "Create Application". Skip ghost / cancel
  // buttons.
  for (const b of dialog.querySelectorAll<HTMLButtonElement>("button")) {
    const t = b.textContent?.trim() ?? "";
    if (/^create\b/i.test(t)) return b;
  }
  return null;
}

/** Minimal CSS.escape polyfill — handles the common case (no
 *  spaces, no colons, no special chars in Radix-generated ids). */
function cssEscape(s: string): string {
  if (typeof CSS !== "undefined" && CSS.escape) return CSS.escape(s);
  return s.replace(/([!"#$%&'()*+,./:;<=>?@[\\\]^`{|}~])/g, "\\$1");
}

// =================================================================
// State logic
// =================================================================
function nameIsComplete(input: HTMLElement): boolean {
  return input instanceof HTMLInputElement && input.value.length > 0;
}

function envIsProduction(selector: HTMLElement): boolean {
  // For native <select>: read .value or selected option text.
  if (selector instanceof HTMLSelectElement) {
    const v = selector.value;
    if (/production/i.test(v)) return true;
    const sel = selector.options[selector.selectedIndex];
    return !!sel && /^production$/i.test(sel.text.trim());
  }
  // For Radix combobox button: textContent shows the current
  // selection. May include extra characters (chevron icons) —
  // check for "production" substring case-insensitive.
  const t = selector.textContent?.trim() ?? "";
  // Tighten: must contain "Production" *as a word*, not a
  // substring of something else.
  return /\bproduction\b/i.test(t);
}

// =================================================================
// Mount / unmount lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
const widgets = new Map<DialogStep["id"], HelperWidget>();
let rafId: number | null = null;
let urlUnsubscribe: (() => void) | null = null;

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_create_app_dialog_helpers";
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
    const widget = createHelperWidget({ text: step.text, arrow: "right" });
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
  // Find the dialog. If absent, hide all widgets and keep ticking.
  const dialog = findDialog();
  if (!dialog) {
    for (const w of widgets.values()) w.element.style.display = "none";
    rafId = requestAnimationFrame(tick);
    return;
  }

  // First pass: compute env state (drives the Create button's
  // blocked state).
  const envTarget = STEPS[1].getTarget(dialog);
  const envIsProd = !!envTarget && envIsProduction(envTarget);

  for (const step of STEPS) {
    const widget = widgets.get(step.id);
    if (!widget) continue;
    const el = widget.element;
    const target = step.getTarget(dialog);
    if (!target) {
      el.style.display = "none";
      continue;
    }
    el.style.display = "";

    // Anchor to the LEFT of the target (matches onboarding +
    // page Create pointer conventions). `translateX(-100%)` puts
    // the helper's right edge against the chosen left coord.
    const rect = target.getBoundingClientRect();
    el.style.top = `${rect.top + rect.height / 2}px`;
    el.style.left = `${rect.left - 8}px`;
    el.style.transform = "translateX(-100%) translateY(-50%)";

    // Cap max-width to fit on the left side (same trick as the
    // onboarding helpers — wrap when narrow).
    const GAP = 8;
    const VIEWPORT_MARGIN = 12;
    const MIN_WIDTH = 120;
    const MAX_WIDTH = 300;
    const available = rect.left - GAP - VIEWPORT_MARGIN;
    el.style.maxWidth = `${Math.max(
      MIN_WIDTH,
      Math.min(MAX_WIDTH, available),
    )}px`;

    // State per step.
    let state: HelperState;
    switch (step.id) {
      case "name":
        state = nameIsComplete(target) ? "complete" : "incomplete";
        break;
      case "env":
        state = envIsProd ? "complete" : "incomplete";
        break;
      case "create":
        // Mirror the onboarding Submit pattern: blocked (red+✕)
        // until prereqs (env=Production) met, then neutral.
        // Never reaches "complete" — clicking closes the dialog.
        state = envIsProd ? "incomplete" : "blocked";
        break;
    }
    widget.setState(state);
  }
  rafId = requestAnimationFrame(tick);
}

function findDialog(): HTMLElement | null {
  if (!isCreateAppDialogOpen()) return null;
  for (const d of document.querySelectorAll<HTMLElement>('[role="dialog"]')) {
    const heading = d.querySelector<HTMLElement>(
      'h1, h2, h3, [role="heading"]',
    );
    if (heading?.textContent?.trim() === "Create New Client Application") {
      return d;
    }
  }
  return null;
}

// =================================================================
// Public entry point
// =================================================================
export function installCreateAppDialogHelpers(): () => void {
  urlUnsubscribe = subscribeUrl((url) => {
    if (isOnAppsTab(url)) {
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
