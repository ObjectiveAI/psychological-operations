// Mirror of the Rust-side `PanelState` inside the content overlay.
//
// Rust calls `cef::execute_overlay_js("window.__psyops_set_panel(<json>)")`
// from `state::recompute_and_publish` every time the panel changes —
// see `src-tauri/src/state.rs`. This module installs the receiver, holds
// the most recent value, and exposes synchronous reads for the per-pointer
// helpers so they can render in lockstep with the panel.

export type DiscordField = { value?: string; saving: boolean };
export type DiscordAuthForm = {
  application_id: DiscordField;
  public_key: DiscordField;
  bot_token: DiscordField;
};

export type PanelState =
  | { type: "show"; condition: string; message: string }
  | { type: "hidden" }
  // Rust's `DiscordAuth(DiscordAuthForm)` is an internally-tagged newtype
  // variant, so the form fields sit alongside `type` on the wire.
  | ({ type: "discord_auth" } & DiscordAuthForm);

let current: PanelState | null = null;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
(window as any).__psyops_set_panel = (s: PanelState | null) => {
  current = s;
};

/** Current panel state, or `null` if nothing has been observed yet. */
export function getPanelState(): PanelState | null {
  return current;
}

/** Convenience: true iff the panel is currently in the
 *  `show`-with-this-condition state. Used by per-pointer helpers to gate
 *  visibility on the same fact the panel header uses. */
export function isPanelCondition(cond: string): boolean {
  return current?.type === "show" && current.condition === cond;
}

/** The DiscordLogin auth form, when the panel is showing it. Lets the
 *  Discord pointer modules gate on which fields are already captured. */
export function getDiscordAuth(): DiscordAuthForm | null {
  return current?.type === "discord_auth" ? current : null;
}
