//! Panel-condition wire types.
//!
//! The browser's instruction panel is driven by a single derived
//! [`PanelState`] published by the Rust side. Watchers contribute raw
//! facts (cookies, mode, …); a pure derivation in the browser's
//! `state` module turns them into a [`PanelState`]. That state goes
//! out three places — stdout (as [`crate::browser::output::Output::Panel`]),
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
/// `click_apps_tab`, `click_create_app`, `click_production_app`.
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
    /// X-App mode, on the Apps tab, but the user needs to create
    /// a new app — either no first-three creds yet, or first three
    /// present + no production app in the list (we fall back to
    /// the create flow). Pair with the in-page pointer at the
    /// "Create App" button.
    ClickCreateApp,
    /// X-App mode, on the Apps tab, the first three creds are
    /// on disk but the access tokens aren't, and the user has
    /// at least one production app. Pair with the in-page
    /// pointer at the first production app's row.
    ClickProductionApp,
    /// X-App mode, but `derive` doesn't yet have enough facts
    /// to pick a real condition — typically the brief window
    /// before the first cookies snapshot lands, or while the
    /// apps-page count is still debouncing. The panel React
    /// renders this as three flashing dots so the user sees a
    /// "thinking" affordance rather than the panel collapsing
    /// to zero height. The `message` is empty by convention.
    Loading,
    /// X-App mode, on an individual app's overview page
    /// (`/accounts/<id>/apps/<app-id>`, no further segments),
    /// the first three creds are on disk but the access
    /// tokens aren't. Pair with the in-page pointer at the
    /// "Settings" tab — the next step toward generating
    /// those tokens.
    ClickSettings,
    /// X-App mode, on the dev console's auth-settings page
    /// (`/accounts/<id>/apps/settings?appId=<app-id>`), the
    /// first three creds are on disk but the access tokens
    /// aren't. Pair with the multi-badge wizard that walks
    /// the user through the App permissions / Type of App /
    /// Website URL / Callback URI fields and the Save Changes
    /// button.
    ConfigureAuthSettings,
    /// AgentRead mode, signed in — show the running counter
    /// "Tweets read: X" where X is the
    /// unique tweet-ID count for this session. The count
    /// lives in [`PanelState::Show::message`]; this variant
    /// is just the stable identifier so renderers can pick
    /// a non-text affordance (e.g. a progress chip) if they
    /// want.
    TweetsRead,
}

/// One field of the DiscordLogin auth form (see [`DiscordAuthForm`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordField {
    /// The saved value, if captured. `None` ⇒ not captured yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A capture is in flight (writing to the DB) → render loading dots.
    pub saving: bool,
}

/// The DiscordLogin top-header auth form — the three credential values the
/// wizard captures, mirrored from the DB and shown read-only for the whole
/// Discord session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordAuthForm {
    pub application_id: DiscordField,
    pub public_key: DiscordField,
    pub bot_token: DiscordField,
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
    /// DiscordLogin mode — a persistent read-only auth form (Application
    /// ID / Public Key / Bot Token) shown for the whole session.
    DiscordAuth(DiscordAuthForm),
}

impl PanelState {
    /// True when the panel should occupy its full row (i.e. there's
    /// something to render). Used by the reflow logic.
    pub fn is_visible(&self) -> bool {
        matches!(self, PanelState::Show { .. } | PanelState::DiscordAuth(_))
    }
}
