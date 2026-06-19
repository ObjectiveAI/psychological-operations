// AgentRead overlay helper — Save button.
//
// Installed by main.tsx when `current_mode().type === "agent_read"`.
// Renders a fixed-position "Save" button near the bottom-center of
// the page. Clicking it snapshots the current page HTML and ships
// it to Rust via `invoke("process_read_html", { html })`. From
// Rust's perspective the call lands at exactly the same handler
// the prior rAF auto-poller used (`agent_read::process_html`) —
// only the trigger model changes.
//
// Visual feedback rotates the button label "Save" → "Saving…" →
// "Saved" → "Save" so the user has a clear acknowledgement that
// the click did something.
//
// Gating: the click is a no-op when the panel is in
// `sign_in_to_x` — that's the state where ingesting HTML would be
// impossible (no session).

import { invoke } from "./ipc";
import { isPanelCondition } from "./panel-state";

const BUTTON_TEXT_IDLE = "Save";
const BUTTON_TEXT_INFLIGHT = "Saving…";
const BUTTON_TEXT_DONE = "Saved";
const DONE_HOLD_MS = 1000;

const ROOT_CSS = `
:host {
  all: initial;
}
.btn {
  position: fixed;
  bottom: 24px;
  left: 50%;
  transform: translateX(-50%);
  pointer-events: auto;
  z-index: 2147483600;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
  font-size: 14px;
  font-weight: 600;
  letter-spacing: 0.02em;
  color: #f8fafc;
  background: rgba(15, 23, 42, 0.92);
  border: 1px solid rgba(148, 163, 184, 0.35);
  border-radius: 999px;
  padding: 12px 28px;
  min-width: 140px;
  cursor: pointer;
  box-shadow:
    0 1px 0 rgba(255, 255, 255, 0.06) inset,
    0 8px 24px rgba(0, 0, 0, 0.45);
  transition: background 120ms ease, transform 120ms ease, opacity 120ms ease;
}
.btn:hover:not(:disabled) {
  background: rgba(30, 41, 59, 0.95);
}
.btn:active:not(:disabled) {
  transform: translateX(-50%) translateY(1px);
}
.btn:disabled {
  cursor: default;
  opacity: 0.55;
}
.btn.is-done {
  background: rgba(22, 101, 52, 0.92);
  border-color: rgba(74, 222, 128, 0.55);
}
`.trim();

export function installAgentReadHelpers(): () => void {
  const container = document.createElement("div");
  container.style.cssText =
    "position:fixed;width:0;height:0;pointer-events:none;z-index:2147483600;left:0;top:0;";
  const shadow = container.attachShadow({ mode: "closed" });

  const style = document.createElement("style");
  style.textContent = ROOT_CSS;
  shadow.appendChild(style);

  const btn = document.createElement("button");
  btn.className = "btn";
  btn.type = "button";
  btn.textContent = BUTTON_TEXT_IDLE;
  shadow.appendChild(btn);

  document.documentElement.appendChild(container);

  let inFlight = false;
  let resetTimer: number | null = null;

  const updateDisabled = () => {
    const gated = isPanelCondition("sign_in_to_x");
    btn.disabled = gated || inFlight;
  };

  // Re-evaluate disable state on a short interval — the panel
  // mirror is updated by Rust via window.__psyops_set_panel,
  // but we don't get a synchronous notification of the change,
  // so a periodic re-check keeps the button visually in sync.
  const gateInterval = window.setInterval(updateDisabled, 250);
  updateDisabled();

  btn.addEventListener("click", async () => {
    if (btn.disabled) return;
    if (resetTimer !== null) {
      window.clearTimeout(resetTimer);
      resetTimer = null;
    }
    inFlight = true;
    btn.classList.remove("is-done");
    btn.textContent = BUTTON_TEXT_INFLIGHT;
    updateDisabled();
    try {
      const html = document.documentElement.outerHTML;
      await invoke<number>("process_read_html", { html });
      btn.textContent = BUTTON_TEXT_DONE;
      btn.classList.add("is-done");
    } catch (e) {
      console.warn("[psyops-overlay] save failed:", e);
      btn.textContent = BUTTON_TEXT_IDLE;
    } finally {
      inFlight = false;
      updateDisabled();
      resetTimer = window.setTimeout(() => {
        btn.textContent = BUTTON_TEXT_IDLE;
        btn.classList.remove("is-done");
        resetTimer = null;
      }, DONE_HOLD_MS);
    }
  });

  return () => {
    window.clearInterval(gateInterval);
    if (resetTimer !== null) {
      window.clearTimeout(resetTimer);
      resetTimer = null;
    }
    container.remove();
  };
}
