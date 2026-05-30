// Shared helper-badge widget used by every "overlay pointer"
// module (onboarding form helpers, apps-tab "click here" pointer,
// and whatever future wizard steps land). Owns the visual
// language (dark pill, white text, status circle, optional Copy
// button) and the state-driven render. Consumers own positioning
// — append `widget.element` into your own shadow root and set
// `.style.left / .top / .transform` from your own tick loop.
//
// TOS-safe API surface (carried over from the original onboarding
// helpers):
//   - DOM creation under YOUR root (no page mutation)
//   - addEventListener on the widget's Copy button (user click)
//   - navigator.clipboard.writeText on user click
//
// Deliberately ABSENT (grep this file to confirm):
//   `.value =`, `.checked =`, `.click()`, `.dispatchEvent`,
//   `fetch`, `XMLHttpRequest`.

/**
 * Visual state a helper badge can be in.
 *
 *   - `incomplete` — gray-outlined circle, no icon. Default.
 *   - `complete`   — green background, white-on-green ✓.
 *   - `blocked`    — red background, white-on-red ✕. Used for
 *                    "you can't act on this yet" states (e.g.
 *                    the onboarding Submit step before its
 *                    prerequisites are all green).
 */
export type HelperState = "incomplete" | "complete" | "blocked";

export type HelperOptions = {
  /** Instruction text inside the badge. */
  text: string;
  /** If set, render a Copy button next to the text; on click,
   *  `copyText` lands on the OS clipboard (and the button
   *  briefly flips to a "Copied!" confirmation). */
  copyText?: string;
  /** Override the Copy button's label. Default: "Copy". */
  copyButtonLabel?: string;
  /** Render a small triangle protruding from this side of the
   *  badge, pointing at the target the consumer placed the badge
   *  next to. Default: no arrow. */
  arrow?: "left" | "right";
};

export interface HelperWidget {
  /** The DOM element to position + display. Owned by the caller
   *  — append it to your shadow root and set its position styles
   *  from your tick loop. */
  readonly element: HTMLElement;
  setState(state: HelperState): void;
  setText(text: string): void;
}

/** CSS for every helper-widget consumer's shadow root. Wrap in a
 *  `<style>` tag and append once on mount. */
export const HELPER_CSS = `
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
    border: 1.5px solid rgba(91, 148, 255, 0.95);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
    pointer-events: auto;
    transition: background 180ms ease, border-color 180ms ease;
    /* Wrap text when narrow. The consumer is responsible for
       capping max-width per-tick to whatever room is available
       so the badge does not extend past the viewport. */
    white-space: normal;
    overflow-wrap: anywhere;
    /* Border pulses through colors AND alpha-flashes in/out
       to read as "alive / awaiting input". The .complete and
       .blocked overrides below halt the animation so the
       border sticks to its definitive green/red. */
    animation: psyops-helper-pulse 3s linear infinite;
  }
  .helper.complete {
    background: rgba(34, 139, 60, 0.95);
    border-color: rgba(120, 220, 150, 0.6);
    animation: none;
  }
  .helper.blocked {
    background: rgba(180, 40, 40, 0.95);
    border-color: rgba(255, 130, 130, 0.6);
    animation: none;
  }
  @keyframes psyops-helper-pulse {
    0%    { border-color: rgba(91, 148, 255, 0.95); }
    12.5% { border-color: rgba(91, 148, 255, 0.20); }
    25%   { border-color: rgba(183, 91, 255, 0.95); }
    37.5% { border-color: rgba(183, 91, 255, 0.20); }
    50%   { border-color: rgba(255, 91, 183, 0.95); }
    62.5% { border-color: rgba(255, 91, 183, 0.20); }
    75%   { border-color: rgba(255, 183, 91, 0.95); }
    87.5% { border-color: rgba(255, 183, 91, 0.20); }
    100%  { border-color: rgba(91, 148, 255, 0.95); }
  }
  .helper .status {
    /* Hidden in the incomplete state — the empty outlined
       circle carries no information. The complete / blocked
       overrides below flip it back on once it has a
       meaningful glyph (✓ or ✕) to show. */
    display: none;
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
    display: inline-flex;
    background: #fff;
    border-color: #fff;
    color: #1a7a3a;
  }
  .helper.blocked .status {
    display: inline-flex;
    background: #fff;
    border-color: #fff;
    color: #b32828;
  }
  /* Optional speech-bubble-tail arrow: opt-in via the 'arrow'
     option on createHelperWidget. The 8px triangle exactly
     fills the 8px gap each consumer leaves between badge and
     target — tip lands at the target's edge. A colored
     drop-shadow gives the triangle a halo so it stays
     visible against dark page backgrounds. */
  .helper.arrow-left::before,
  .helper.arrow-right::after {
    content: "";
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    border: 8px solid transparent;
    pointer-events: none;
    transition: border-color 180ms ease, filter 180ms ease;
    filter: drop-shadow(0 0 3px rgba(91, 148, 255, 0.85));
  }
  .helper.arrow-left::before {
    right: 100%;
    border-right-color: rgba(20, 25, 35, 0.95);
  }
  .helper.arrow-right::after {
    left: 100%;
    border-left-color: rgba(20, 25, 35, 0.95);
  }
  .helper.complete.arrow-left::before {
    border-right-color: rgba(34, 139, 60, 0.95);
    filter: drop-shadow(0 0 3px rgba(120, 220, 150, 0.85));
  }
  .helper.complete.arrow-right::after {
    border-left-color: rgba(34, 139, 60, 0.95);
    filter: drop-shadow(0 0 3px rgba(120, 220, 150, 0.85));
  }
  .helper.blocked.arrow-left::before {
    border-right-color: rgba(180, 40, 40, 0.95);
    filter: drop-shadow(0 0 3px rgba(255, 130, 130, 0.85));
  }
  .helper.blocked.arrow-right::after {
    border-left-color: rgba(180, 40, 40, 0.95);
    filter: drop-shadow(0 0 3px rgba(255, 130, 130, 0.85));
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

/** Build a new helper badge. The element starts in
 *  `incomplete` state with the given text; the caller can flip
 *  state and text any time via the returned controller. */
export function createHelperWidget(opts: HelperOptions): HelperWidget {
  const root = document.createElement("div");
  root.className = "helper";
  if (opts.arrow === "left") {
    root.classList.add("arrow-left");
  } else if (opts.arrow === "right") {
    root.classList.add("arrow-right");
  }

  const textEl = document.createElement("span");
  textEl.textContent = opts.text;
  root.appendChild(textEl);

  if (opts.copyText !== undefined) {
    const label = opts.copyButtonLabel ?? "Copy";
    const copyText = opts.copyText;
    const btn = document.createElement("button");
    btn.className = "copy-btn";
    btn.type = "button";
    btn.textContent = label;
    btn.addEventListener("click", () => {
      navigator.clipboard
        .writeText(copyText)
        .then(() => {
          btn.classList.add("copied");
          btn.textContent = "Copied!";
          setTimeout(() => {
            btn.classList.remove("copied");
            btn.textContent = label;
          }, 1400);
        })
        .catch(() => {
          btn.textContent = "Copy failed";
          setTimeout(() => {
            btn.textContent = label;
          }, 1400);
        });
    });
    root.appendChild(btn);
  }

  const statusEl = document.createElement("span");
  statusEl.className = "status";
  root.appendChild(statusEl);

  return {
    element: root,
    setState(state) {
      root.classList.toggle("complete", state === "complete");
      root.classList.toggle("blocked", state === "blocked");
      statusEl.textContent =
        state === "complete" ? "✓" : state === "blocked" ? "✕" : "";
    },
    setText(t) {
      textEl.textContent = t;
    },
  };
}
