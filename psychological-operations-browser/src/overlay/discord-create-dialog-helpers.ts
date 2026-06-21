// "Create a new app" dialog helpers (Mode::DiscordLogin) — three badges
// guiding the operator through the dialog's controls:
//
//   1. App name      → copy badge carrying the agent tag (paste it in)
//   2. ToS checkbox  → "Click here" (accept the Developer Terms)
//   3. Create button → "Click here", red+✕ until ToS accepted AND the name
//                      matches the tag (the "correct content"), then neutral
//
// Mirrors the X create-app-dialog pattern: anchored badges via the shared
// `helper-widget`, blocked Create until prereqs met, green checkmarks. Runs
// only while the dialog is open. The simple-pointer wizard
// (discord-login-helpers) gates its create_app pointer off via
// `isDiscordCreateDialogOpen` so the two never overlap.
//
// TOS posture: read-only DOM observation + render under our own shadow root.
// No `.value=`, `.checked=`, `.click()`, `.dispatchEvent`, or fetch.

import {
  HELPER_CSS,
  createHelperWidget,
  type HelperState,
  type HelperWidget,
} from "./helper-widget";

/** The "Create a new app" dialog, if open. Exported so the simple-pointer
 *  wizard can suppress its New Application pointer while this is up. */
export function isDiscordCreateDialogOpen(): boolean {
  return findDialog() !== null;
}

function findDialog(): HTMLElement | null {
  for (const d of document.querySelectorAll<HTMLElement>('[role="dialog"]')) {
    const h = d.querySelector<HTMLElement>('h1, h2, h3, [role="heading"]');
    if ((h?.textContent ?? "").trim().toLowerCase() === "create a new app") {
      return d;
    }
  }
  return null;
}

// --- target finders (scoped to the dialog) -------------------------------
function findNameInput(d: HTMLElement): HTMLInputElement | null {
  return (
    d.querySelector<HTMLInputElement>('input[name="name"]') ??
    [...d.querySelectorAll<HTMLInputElement>("input")].find(
      (i) => (i.getAttribute("type") ?? "text").toLowerCase() === "text",
    ) ??
    null
  );
}

function findTosCheckbox(d: HTMLElement): HTMLInputElement | null {
  return d.querySelector<HTMLInputElement>('input[type="checkbox"]');
}

function findCreateButton(d: HTMLElement): HTMLButtonElement | null {
  for (const b of d.querySelectorAll<HTMLButtonElement>("button")) {
    if (/^create\b/i.test((b.textContent ?? "").trim())) return b;
  }
  return null;
}

// --- state ---------------------------------------------------------------
/** The name field holds exactly the agent tag (the "correct content"). */
function nameMatchesTag(input: HTMLInputElement, tag: string): boolean {
  return input.value.trim() === tag;
}

type StepId = "name" | "tos" | "create";

let agentTag = "";
let rootEl: HTMLDivElement | null = null;
const widgets = new Map<StepId, HelperWidget>();
let rafId: number | null = null;

function mount(): void {
  if (rootEl) return;

  rootEl = document.createElement("div");
  rootEl.id = "__psyops_discord_create_dialog_helpers";
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

  // Name: a copy badge carrying the tag. The others: plain "Click here".
  // All anchored to the LEFT of their target (arrow on the right edge).
  widgets.set(
    "name",
    createHelperWidget({
      text: agentTag,
      copyText: agentTag,
      copyButtonLabel: "Copy",
      arrow: "right",
    }),
  );
  widgets.set("tos", createHelperWidget({ text: "Click here", arrow: "right" }));
  widgets.set("create", createHelperWidget({ text: "Click here", arrow: "right" }));
  for (const w of widgets.values()) {
    w.element.style.display = "none";
    shadow.appendChild(w.element);
  }

  document.body.appendChild(rootEl);
  rafId = requestAnimationFrame(tick);
}

function place(el: HTMLElement, target: HTMLElement): void {
  const rect = target.getBoundingClientRect();
  el.style.top = `${rect.top + rect.height / 2}px`;
  el.style.left = `${rect.left - 8}px`;
  el.style.transform = "translate(-100%, -50%)";
  // Cap width to the room on the left so the badge never clips off-screen.
  const available = rect.left - 8 - 12;
  el.style.maxWidth = `${Math.max(120, Math.min(300, available))}px`;
}

function tick(): void {
  const dialog = findDialog();
  if (!dialog) {
    for (const w of widgets.values()) w.element.style.display = "none";
    rafId = requestAnimationFrame(tick);
    return;
  }

  const nameEl = findNameInput(dialog);
  const tosEl = findTosCheckbox(dialog);
  const createEl = findCreateButton(dialog);

  const nameOk = !!nameEl && nameMatchesTag(nameEl, agentTag);
  const tosOk = !!tosEl && tosEl.checked;

  // name badge
  const nameW = widgets.get("name")!;
  if (nameEl) {
    nameW.element.style.display = "";
    place(nameW.element, nameEl);
    nameW.setState(nameOk ? "complete" : "incomplete");
  } else {
    nameW.element.style.display = "none";
  }

  // tos badge
  const tosW = widgets.get("tos")!;
  if (tosEl) {
    tosW.element.style.display = "";
    place(tosW.element, tosEl);
    tosW.setState(tosOk ? "complete" : "incomplete");
  } else {
    tosW.element.style.display = "none";
  }

  // create badge — red+✕ until both prereqs met, then neutral (clicking
  // closes the dialog, so it never goes green).
  const createW = widgets.get("create")!;
  if (createEl) {
    createW.element.style.display = "";
    place(createW.element, createEl);
    const state: HelperState = nameOk && tosOk ? "incomplete" : "blocked";
    createW.setState(state);
  } else {
    createW.element.style.display = "none";
  }

  rafId = requestAnimationFrame(tick);
}

/** Install the create-dialog helpers for `tag` (the agent the bot is for —
 *  its name goes in the copy badge and gates Create). */
export function installDiscordCreateDialogHelpers(tag: string): () => void {
  agentTag = tag;
  mount();
  return () => {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    rootEl?.remove();
    rootEl = null;
    widgets.clear();
  };
}
