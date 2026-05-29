// Onboarding-form helpers for `console.x.com/onboarding`.
//
// Six small badges, each anchored to a form element, that flip
// green-with-checkmark when the user completes that step. The
// description badge also exposes a "Copy" button that writes the
// canonical description string to the OS clipboard. Visuals come
// from the shared `helper-widget` module so this stays consistent
// with other wizard pointers (apps-tab, etc.).
//
// =================================================================
// TOS / "no automation" constraint — enforced by what we use:
// =================================================================
//
//   READ-ONLY APIs we depend on:
//     - addEventListener  (observe user actions, never dispatch)
//     - getBoundingClientRect, querySelector, querySelectorAll
//     - reading element.value / .checked / .textContent
//     - requestAnimationFrame
//     - DOM creation under our own root (via helper-widget) + the
//       Copy button's `navigator.clipboard.writeText` call
//
//   APIs this module is *deliberately forbidden* from using
//   (grep these names in the diff if reviewing this file):
//     - `.value = …`         on any page element
//     - `.checked = …`       on any page element
//     - `.click()`           on any page element
//     - `.dispatchEvent(…)`  on any page element
//     - `.submit()`          on any form
//     - fetch / XMLHttpRequest to x.com / api.x.com
//
// The user does every typing/clicking/submitting action
// themselves. This module renders visual hints + observes the
// resulting state. That's accessibility-tool territory, not
// automation.

import {
  HELPER_CSS,
  createHelperWidget,
  type HelperState,
  type HelperWidget,
} from "./helper-widget";
import { subscribeUrl } from "./spa-url";

// =================================================================
// Canonical description — the textarea must match this exactly
// (modulo trimmed whitespace) for the description step to flip
// to "complete". The "Copy" button writes this to the clipboard.
// =================================================================
const DESCRIPTION_COPY =
  "Create an app which allows clients to use the X API via `psychological-operations` (https://github.com/ObjectiveAI/psychological-operations)";

// =================================================================
// Step definitions
// =================================================================
type Step = {
  id: string;
  text: string;
  hasCopyButton?: true;
  getTarget(): HTMLElement | null;
  isComplete(el: HTMLElement): boolean;
};

const STEPS: Step[] = [
  {
    id: "account-name",
    text: "Enter anything",
    getTarget: () =>
      document.querySelector<HTMLInputElement>("#account-name-input"),
    isComplete: (el) => (el as HTMLInputElement).value.length > 0,
  },
  {
    id: "description",
    text: "Click Copy then paste here",
    hasCopyButton: true,
    getTarget: () =>
      document.querySelector<HTMLTextAreaElement>("#use-case-textarea"),
    isComplete: (el) =>
      (el as HTMLTextAreaElement).value.trim() === DESCRIPTION_COPY.trim(),
  },
  {
    id: "agree-no-resell",
    text: "Check this",
    getTarget: () =>
      document.querySelector<HTMLInputElement>("#agree-no-resell"),
    isComplete: (el) => (el as HTMLInputElement).checked,
  },
  {
    id: "accept-terms",
    text: "Check this",
    getTarget: () => document.querySelector<HTMLInputElement>("#accept-terms"),
    isComplete: (el) => (el as HTMLInputElement).checked,
  },
  {
    id: "agree-termination",
    text: "Check this",
    getTarget: () =>
      document.querySelector<HTMLInputElement>("#agree-termination"),
    isComplete: (el) => (el as HTMLInputElement).checked,
  },
  {
    id: "submit",
    text: "Click this",
    getTarget: () => {
      for (const b of document.querySelectorAll<HTMLButtonElement>("button")) {
        if (b.textContent?.trim() === "Submit") return b;
      }
      return null;
    },
    // Submit doesn't have a "post-click" state — page navigates
    // away on success, which fires our URL subscription and
    // unmounts us. So this stays false.
    isComplete: () => false,
  },
];

// =================================================================
// URL gating
// =================================================================
function isOnboardingUrl(url: string): boolean {
  try {
    const u = new URL(url);
    return u.host === "console.x.com" && u.pathname.startsWith("/onboarding");
  } catch {
    return false;
  }
}

// =================================================================
// Mount / unmount lifecycle
// =================================================================
let rootEl: HTMLDivElement | null = null;
const widgets = new Map<string, HelperWidget>();
let rafId: number | null = null;

function mount() {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_onboarding_helpers";
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
      copyText: step.hasCopyButton ? DESCRIPTION_COPY : undefined,
      arrow: "right",
    });
    widget.element.dataset.step = step.id;
    widgets.set(step.id, widget);
    shadow.appendChild(widget.element);
  }

  document.body.appendChild(rootEl);
  startTickLoop();
}

function unmount() {
  if (!rootEl) return;
  stopTickLoop();
  rootEl.remove();
  rootEl = null;
  widgets.clear();
}

// =================================================================
// Per-frame tick — re-finds targets, repositions, refreshes
// completion state. Polling rather than event listeners because:
//
//   1. React may re-render and swap element references, which
//      would orphan any listener we attached to the old element.
//   2. Six selectors + six bounding-rect reads + six property
//      checks per frame is trivially cheap.
//   3. Position-on-scroll/resize and state-on-change collapse to
//      the same loop. Less wiring.
//
// Throttled to display refresh by `requestAnimationFrame`.
// =================================================================
function tick() {
  // First pass: compute completion of every step *except* submit.
  // The submit helper is "blocked" (red + X) until all the other
  // steps are complete — it doesn't make sense to invite a click
  // before the form is fillable.
  const nonSubmitComplete = STEPS.every((s) => {
    if (s.id === "submit") return true;
    const t = s.getTarget();
    return t ? s.isComplete(t) : false;
  });

  for (const step of STEPS) {
    const widget = widgets.get(step.id);
    if (!widget) continue;
    const el = widget.element;

    const target = step.getTarget();
    if (!target) {
      el.style.display = "none";
      continue;
    }
    el.style.display = "";

    // Position to the left of the target. For tall fields (like
    // the description textarea) anchor near the top; for short
    // fields (inputs, checkboxes) vertically center. `translateX
    // (-100%)` places the helper's right edge at the chosen left
    // coordinate, so we don't have to measure helper width.
    //
    // Also cap max-width per-tick to whatever room is available
    // between the viewport's left edge and the field, so when the
    // window is narrow the helper wraps to multiple lines instead
    // of clipping off-screen. Floor at MIN_WIDTH so the helper
    // stays legible even at extreme widths (it'll sliver off the
    // edge a bit rather than vanish).
    const rect = target.getBoundingClientRect();
    const GAP = 8;
    const VIEWPORT_MARGIN = 12;
    const MIN_WIDTH = 120;
    const MAX_WIDTH = 300;
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

    // Compute the visual state for this step + push to the
    // shared widget.
    let state: HelperState;
    if (step.id === "submit") {
      // Three states for submit: blocked (red + X) when prereqs
      // aren't met, neutral (gray circle) when ready to click,
      // never "complete" (page navigates on click → unmount).
      state = nonSubmitComplete ? "incomplete" : "blocked";
    } else {
      state = step.isComplete(target) ? "complete" : "incomplete";
    }
    widget.setState(state);
  }
  rafId = requestAnimationFrame(tick);
}

function startTickLoop() {
  if (rafId !== null) return;
  rafId = requestAnimationFrame(tick);
}

function stopTickLoop() {
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
}

// =================================================================
// Public entry point
// =================================================================
let urlUnsubscribe: (() => void) | null = null;

/**
 * Install onboarding helpers on the content webview. Returns an
 * uninstall closure. Mounts/unmounts the helpers automatically on
 * every URL change — visible only on `console.x.com/onboarding`.
 */
export function installOnboardingHelpers(): () => void {
  urlUnsubscribe = subscribeUrl((url) => {
    if (isOnboardingUrl(url)) {
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
