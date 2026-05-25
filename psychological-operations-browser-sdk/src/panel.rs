//! Panel-condition wire types.
//!
//! The browser's instruction panel is driven by a single derived
//! [`PanelState`] published by the Rust side. Watchers contribute raw
//! facts (cookies, mode, …); a pure derivation in the browser's
//! `state` module turns them into a [`PanelState`]. That state goes
//! out three places — stdout (as [`crate::output::Output::Panel`]),
//! the panel webview (via Tauri event), and the reflow logic that
//! resizes the panel to either `0` or `PANEL_HEIGHT`.
//!
//! Adding a new condition: add a [`PanelCondition`] variant + a
//! `derive` arm. The panel React + reflow auto-pick it up.

use serde::{Deserialize, Serialize};

/// Stable identifier for each panel-shown condition. The exact
/// message text lives in [`PanelState::Show::message`] and is allowed
/// to change without breaking callers that act on the condition.
///
/// Snake-case on the wire — `sign_in_to_x`, `needs_x_app_setup`,
/// `click_apps_tab`, `click_create_app`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelCondition {
    /// X-App mode, not signed in to x.com (no `auth_token` cookie).
    SignInToX,
    /// X-App mode, signed in, content webview is on
    /// `console.x.com/onboarding` — user hasn't yet completed the
    /// app-setup flow.
    NeedsXAppSetup,
    /// X-App mode, signed in, past onboarding, but the content
    /// webview is not on the Apps tab (`/accounts/<id>/apps[/...]`)
    /// yet. Pair with the in-page sidebar pointer that points at
    /// the Apps sidebar link.
    ClickAppsTab,
    /// X-App mode, on the Apps tab, but the overlay has reported
    /// zero production apps. Pair with the in-page pointer at the
    /// "Create App" button.
    ClickCreateApp,
}

/// Everything the panel needs to render. Either it's hidden (zero
/// height, no message) or it's showing a single instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelState {
    Hidden,
    Show {
        condition: PanelCondition,
        message: String,
    },
}

impl PanelState {
    /// True when the panel should occupy its full row (i.e. there's
    /// something to render). Used by the reflow logic.
    pub fn is_visible(&self) -> bool {
        matches!(self, PanelState::Show { .. })
    }
}
