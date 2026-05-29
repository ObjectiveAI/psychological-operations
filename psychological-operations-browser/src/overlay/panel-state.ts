// Mirror of the Rust-side `PanelState` inside the content overlay.
//
// Rust calls `cef::execute_overlay_js("window.__psyops_set_panel(<json>)")`
// from `state::recompute_and_publish` every time the panel changes —
// see `src-tauri/src/state.rs`. This module installs the receiver, holds
// the most recent value, and exposes a synchronous read for the per-
// pointer helpers so they can render in lockstep with the panel header.
//
// The setter is registered at module load so the very first push lands
// safely. main.tsx also seeds the initial value via `invoke("current_panel")`
// at overlay mount, covering the case where the panel was already stable
// before the overlay attached (e.g. signed in + creds-complete from boot,
// so nothing changes for the lifetime of this overlay instance).

export type PanelState =
  | { type: "show"; condition: string; message: string }
  | { type: "hidden" };

let current: PanelState | null = null;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
(window as any).__psyops_set_panel = (s: PanelState | null) => {
  current = s;
};

/** Current panel state, or `null` if nothing has been observed yet
 *  (pre-seed, pre-first-recompute). */
export function getPanelState(): PanelState | null {
  return current;
}

/** Convenience: true iff the panel is currently in the
 *  `show`-with-this-condition state. Used by per-pointer helpers
 *  to gate visibility on the same fact the panel header uses. */
export function isPanelCondition(cond: string): boolean {
  return current?.type === "show" && current.condition === cond;
}
