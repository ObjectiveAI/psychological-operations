// Onboarding-form helpers for `console.x.com/onboarding`.
//
// Six small badges, each anchored to a form element, that flip
// green-with-checkmark when the user completes that step. The
// description badge also exposes a "Copy" button that writes the
// canonical description string to the OS clipboard.
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
//     - navigator.clipboard.writeText (writes OS clipboard; the
//       user pastes themselves)
//     - attachShadow, document.createElement (under our own root)
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
const helperEls = new Map<string, HTMLDivElement>();
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
  shadow.appendChild(makeStyles());
  for (const step of STEPS) {
    const helper = makeHelper(step);
    helperEls.set(step.id, helper);
    shadow.appendChild(helper);
  }

  document.body.appendChild(rootEl);
  startTickLoop();
}

function unmount() {
  if (!rootEl) return;
  stopTickLoop();
  rootEl.remove();
  rootEl = null;
  helperEls.clear();
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
    const helper = helperEls.get(step.id);
    if (!helper) continue;

    const target = step.getTarget();
    if (!target) {
      helper.style.display = "none";
      continue;
    }
    helper.style.display = "";

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
    helper.style.maxWidth = `${Math.max(
      MIN_WIDTH,
      Math.min(MAX_WIDTH, available),
    )}px`;

    if (rect.height > 60) {
      helper.style.top = `${rect.top + 8}px`;
      helper.style.transform = "translateX(-100%)";
    } else {
      helper.style.top = `${rect.top + rect.height / 2}px`;
      helper.style.transform = "translateX(-100%) translateY(-50%)";
    }
    helper.style.left = `${rect.left - GAP}px`;

    // Status / state visualization.
    const status = helper.querySelector<HTMLSpanElement>(".status");
    if (step.id === "submit") {
      // Three states for submit: blocked (red + X) when prereqs
      // aren't met, neutral (gray circle) when ready to click,
      // never "complete" (page navigates on click → unmount).
      const blocked = !nonSubmitComplete;
      helper.classList.toggle("blocked", blocked);
      helper.classList.remove("complete");
      if (status) status.textContent = blocked ? "✕" : "";
    } else {
      const complete = step.isComplete(target);
      helper.classList.toggle("complete", complete);
      if (status) status.textContent = complete ? "✓" : "";
    }
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
// Helper rendering
// =================================================================
function makeStyles(): HTMLStyleElement {
  const s = document.createElement("style");
  s.textContent = `
    .helper {
      position: fixed;
      box-sizing: border-box;
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 6px 10px;
      background: rgba(20, 25, 35, 0.95);
      color: #fff;
      font: 12px/1.35 system-ui, -apple-system, "Segoe UI", sans-serif;
      letter-spacing: 0.01em;
      border: 1px solid rgba(255, 255, 255, 0.18);
      border-radius: 8px;
      box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
      pointer-events: auto;
      transition: background 180ms ease, border-color 180ms ease;
      /* Wrap text when narrow. max-width is set per-tick from
         tick() so the helper never extends past the viewport's
         left edge. overflow-wrap:anywhere catches the case where
         the helper text contains a long unbroken token (URLs,
         etc.) that would otherwise force horizontal overflow. */
      white-space: normal;
      overflow-wrap: anywhere;
    }
    .helper.complete {
      background: rgba(34, 139, 60, 0.95);
      border-color: rgba(120, 220, 150, 0.6);
    }
    .helper.blocked {
      background: rgba(180, 40, 40, 0.95);
      border-color: rgba(255, 130, 130, 0.6);
    }
    .helper .status {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 16px;
      height: 16px;
      border-radius: 50%;
      border: 1.5px solid rgba(255, 255, 255, 0.55);
      flex-shrink: 0;
      font-size: 11px;
      line-height: 1;
      transition: background 180ms ease, border-color 180ms ease, color 180ms ease;
    }
    .helper.complete .status {
      background: #fff;
      border-color: #fff;
      color: #1a7a3a;
    }
    .helper.blocked .status {
      background: #fff;
      border-color: #fff;
      color: #b32828;
    }
    .helper .copy-btn {
      background: rgba(255, 255, 255, 0.14);
      color: #fff;
      border: 1px solid rgba(255, 255, 255, 0.28);
      border-radius: 5px;
      padding: 3px 9px;
      font: inherit;
      cursor: pointer;
      flex-shrink: 0;
      transition: background 120ms ease;
    }
    .helper .copy-btn:hover {
      background: rgba(255, 255, 255, 0.24);
    }
    .helper .copy-btn.copied {
      background: rgba(80, 200, 120, 0.35);
      border-color: rgba(120, 220, 150, 0.6);
    }
  `;
  return s;
}

function makeHelper(step: Step): HTMLDivElement {
  const el = document.createElement("div");
  el.className = "helper";
  el.dataset.step = step.id;

  const text = document.createElement("span");
  text.textContent = step.text;
  el.appendChild(text);

  if (step.hasCopyButton) {
    const btn = document.createElement("button");
    btn.className = "copy-btn";
    btn.type = "button";
    btn.textContent = "Copy";
    btn.addEventListener("click", () => {
      navigator.clipboard
        .writeText(DESCRIPTION_COPY)
        .then(() => {
          btn.classList.add("copied");
          btn.textContent = "Copied!";
          setTimeout(() => {
            btn.classList.remove("copied");
            btn.textContent = "Copy";
          }, 1400);
        })
        .catch(() => {
          btn.textContent = "Copy failed";
          setTimeout(() => {
            btn.textContent = "Copy";
          }, 1400);
        });
    });
    el.appendChild(btn);
  }

  const status = document.createElement("span");
  status.className = "status";
  status.textContent = "";
  el.appendChild(status);

  return el;
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
