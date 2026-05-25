//! Process-global session state — the central fact store + derived
//! [`PanelState`] for the instruction panel.
//!
//! Watchers (and the stdio dispatcher, for mode) contribute raw
//! observations via [`set_mode`], [`set_sso`], [`set_last_team_id`].
//! Each setter updates the [`Facts`] slot and calls
//! [`recompute_and_publish`], which:
//!
//! 1. Runs the pure [`derive`] function over the new facts.
//! 2. Compares the result against the previously-published
//!    [`PanelState`]; bails if unchanged.
//! 3. Publishes the new state four places:
//!    - stdout as [`Output::Panel`]
//!    - the panel webview's Tauri-event listener (`psyops:panel`)
//!    - the X-App window reflow (panel resizes 0 ↔ [`PANEL_HEIGHT`])
//!    - the content webview, via a post-sign-in redirect to
//!      `https://console.x.com/` on the
//!      `SignInToX → !SignInToX` transition.
//!
//! Adding a new panel condition: add a [`PanelCondition`] variant in
//! the SDK + a new fact field here + a new setter + an arm in
//! [`derive`]. The panel React and reflow logic both consume only
//! the derived state — they need no changes.
//!
//! The legacy [`Output::SignedIn`] stream stays for external/CLI
//! consumers: [`set_sso`] emits it on every sso-cookie flip in
//! addition to the panel recompute. [`current_signed_in`] still
//! reports a fresh [`SignedInPayload`] derived from the current sso
//! token.

use std::sync::{Mutex, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use psychological_operations_browser_sdk::mode::{self, Mode};
use psychological_operations_browser_sdk::output::{Output, SignedInInfo};
use psychological_operations_browser_sdk::panel::{PanelCondition, PanelState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, Url};

use crate::webview;

/// Tauri event name fired on every [`PanelState`] change.
const EVENT_PANEL: &str = "psyops:panel";

/// Raw observations watchers contribute. Everything in derived
/// [`PanelState`] is a pure function of these fields.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// Active browser mode (set by stdio dispatch).
    pub mode: Option<Mode>,
    /// x.com's `auth_token` HttpOnly cookie value, if present.
    /// `None` ⇒ signed out. Opaque session string (not a JWT) — see
    /// [`jwt_to_info`].
    pub auth_token: Option<String>,
    /// Most recent URL the content webview's overlay reported.
    /// **Only tracked when `mode` is `Some(Mode::XApp)`** — the
    /// setter is a no-op in other modes, so this field stays
    /// empty for Psyop (etc.) and [`derive`] doesn't have to
    /// special-case those modes. Cleared by [`set_mode`] when
    /// leaving X-App.
    pub current_url: Option<String>,
    /// Count of *production* apps the content overlay observed
    /// in the Apps list. `None` ⇒ the overlay hasn't reported a
    /// count yet (we're not on /apps, or it's still scraping).
    /// `Some(0)` triggers the `ClickCreateApp` panel condition;
    /// `Some(n>0)` keeps the panel hidden. X-App-only, cleared
    /// alongside `current_url` when leaving X-App.
    pub production_app_count: Option<u32>,
    /// X user-id parsed from the `twid` cookie by the cookies
    /// watcher. Stable per signed-in account. Used by the
    /// overlay's per-user credential-storage flow (queried via
    /// the `current_user_id` Tauri command). `None` ⇒ no twid
    /// cookie yet (signed out or pre-snapshot).
    pub user_id: Option<String>,
}

/// Payload for the legacy `current_signed_in` Tauri command +
/// `Output::SignedIn` stdout variant. Kept as a struct (rather than
/// fields on `Facts`) because external consumers depend on this shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedInPayload {
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<SignedInInfo>,
}

// ---------------------------------------------------------------------
// Process-global slots
// ---------------------------------------------------------------------

fn facts_slot() -> &'static Mutex<Facts> {
    static SLOT: OnceLock<Mutex<Facts>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Facts::default()))
}

fn panel_slot() -> &'static Mutex<Option<PanelState>> {
    static SLOT: OnceLock<Mutex<Option<PanelState>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------
// Public read accessors
// ---------------------------------------------------------------------

/// Snapshot of the current derived panel state. `None` before any
/// setter has fired (i.e. fresh process, no facts yet).
pub fn current_panel() -> Option<PanelState> {
    panel_slot()
        .lock()
        .expect("panel slot poisoned")
        .clone()
}

/// Snapshot of the current X user-id (parsed from the `twid`
/// cookie). `None` if cookies haven't been observed yet or the
/// user is signed out.
pub fn current_user_id() -> Option<String> {
    facts_slot()
        .lock()
        .expect("facts slot poisoned")
        .user_id
        .clone()
}

/// Snapshot of the current sign-in state derived from the auth-token
/// cookie fact. `None` before the first panel publication (i.e. no
/// cookie observation has landed yet).
pub fn current_signed_in() -> Option<SignedInPayload> {
    // "Has anything been published yet?" is independent of "what's in
    // the facts store." Reading the panel slot first (and dropping the
    // lock before touching facts) avoids any lock-ordering hazard.
    let panel_observed = panel_slot().lock().expect("panel slot poisoned").is_some();
    if !panel_observed {
        return None;
    }
    let facts = facts_slot().lock().expect("facts slot poisoned");
    Some(SignedInPayload {
        signed_in: facts.auth_token.is_some(),
        info: facts.auth_token.as_deref().and_then(jwt_to_info),
    })
}

// ---------------------------------------------------------------------
// Setters (each triggers a recompute)
// ---------------------------------------------------------------------

/// Mirror a mode change into the facts store. Called from
/// `stdio::dispatch_request` right after [`mode::set`]. One fact,
/// one recompute — safe to use as a standalone setter. Also
/// clears `current_url` when leaving X-App so URL-driven
/// conditions can't fire under a different mode using stale data.
pub fn set_mode<R: Runtime>(handle: &AppHandle<R>, new_mode: Option<Mode>) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if facts.mode == new_mode {
            return;
        }
        facts.mode = new_mode;
        if !matches!(facts.mode, Some(Mode::XApp)) {
            facts.current_url = None;
            facts.production_app_count = None;
            facts.user_id = None;
        }
    }
    recompute_and_publish(handle);
}

/// Update the most-recent-URL fact from `report_url`. Only takes
/// effect in X-App mode; in other modes the call is a no-op so
/// the fact stays empty and [`derive`] is free to ignore it.
pub fn set_current_url<R: Runtime>(handle: &AppHandle<R>, url: String) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if !matches!(facts.mode, Some(Mode::XApp)) {
            return;
        }
        if facts.current_url.as_deref() == Some(url.as_str()) {
            return;
        }
        facts.current_url = Some(url);
    }
    recompute_and_publish(handle);
}

/// Update the production-app count the overlay observed on
/// `/apps`. X-App-only (matches `set_current_url`). Passing
/// `None` clears the fact — used when the overlay leaves /apps
/// so a stale count can't drive `ClickCreateApp` on a different
/// page.
pub fn set_production_app_count<R: Runtime>(
    handle: &AppHandle<R>,
    count: Option<u32>,
) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if !matches!(facts.mode, Some(Mode::XApp)) {
            return;
        }
        if facts.production_app_count == count {
            return;
        }
        facts.production_app_count = count;
    }
    recompute_and_publish(handle);
}

/// Atomically update every cookie-sourced fact from a single
/// observation. Both `auth_token` and `user_id` land under a
/// single lock so no intermediate `PanelState` leaks between them.
/// Emits the legacy [`Output::SignedIn`] line on every auth-token
/// value change before triggering the panel recompute.
pub fn apply_cookie_facts<R: Runtime>(
    handle: &AppHandle<R>,
    auth_token: Option<String>,
    user_id: Option<String>,
) {
    let token_changed_to: Option<Option<String>> = {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        let token_changed = facts.auth_token != auth_token;
        facts.auth_token = auth_token.clone();
        facts.user_id = user_id;
        if token_changed { Some(auth_token) } else { None }
    };

    if let Some(new_token) = token_changed_to {
        let info = new_token.as_deref().and_then(jwt_to_info);
        let _ = Output::SignedIn {
            signed_in: new_token.is_some(),
            info,
        }
        .emit();
    }

    recompute_and_publish(handle);
}

// ---------------------------------------------------------------------
// Derivation + publication
// ---------------------------------------------------------------------

/// Pure mapping from raw facts to the panel state the UI should show.
/// **The heart of the abstraction** — to add a new condition, add a
/// [`PanelCondition`] variant in the SDK and a new arm here.
pub fn derive(facts: &Facts) -> PanelState {
    match facts.mode {
        Some(Mode::XApp) => {
            if facts.auth_token.is_none() {
                return PanelState::Show {
                    condition: PanelCondition::SignInToX,
                    message: "Sign in to X.".into(),
                };
            }
            let url = facts.current_url.as_deref();
            if is_onboarding(url) {
                return PanelState::Show {
                    condition: PanelCondition::NeedsXAppSetup,
                    message: "Set up the X app.".into(),
                };
            }
            if is_apps_tab(url) {
                // On the Apps tab (list or specific app).
                //   Some(0)   → invite the user to create one.
                //   Some(n>0) → they already have one; hidden.
                //   None      → overlay hasn't reported yet —
                //               hidden to avoid flash-of-wrong-
                //               message before the scrape lands.
                if facts.production_app_count == Some(0) {
                    return PanelState::Show {
                        condition: PanelCondition::ClickCreateApp,
                        message: "Click Create App.".into(),
                    };
                }
                return PanelState::Hidden;
            }
            // Signed in, past onboarding, not on the Apps tab —
            // push them to it.
            PanelState::Show {
                condition: PanelCondition::ClickAppsTab,
                message: "Click the Apps tab.".into(),
            }
        }
        _ => PanelState::Hidden,
    }
}

/// True when `url` is on the X-App developer-console onboarding
/// flow (`https://console.x.com/onboarding[/...]`). Restricting
/// the host avoids matching some unrelated `/onboarding` path on
/// x.com proper if the user wanders there.
fn is_onboarding(url: Option<&str>) -> bool {
    let Some(url) = url else { return false };
    let Ok(parsed) = Url::parse(url) else { return false };
    parsed.host_str() == Some("console.x.com")
        && parsed.path().starts_with("/onboarding")
}

/// True when `url` is on the X-App developer-console "Apps" tab
/// — either the apps list (`/accounts/<id>/apps`) or a specific
/// app page (`/accounts/<id>/apps/<app-id>[/...]`).
fn is_apps_tab(url: Option<&str>) -> bool {
    let Some(url) = url else { return false };
    let Ok(parsed) = Url::parse(url) else { return false };
    if parsed.host_str() != Some("console.x.com") {
        return false;
    }
    // Matches /accounts/<digits>/apps and /accounts/<digits>/apps/...
    let path = parsed.path();
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    matches!(segs.next(), Some("accounts"))
        && segs.next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
        && matches!(segs.next(), Some("apps"))
}

/// Re-run [`derive`] on the current facts; if the result differs from
/// the last-published [`PanelState`], publish it everywhere that cares.
pub fn recompute_and_publish<R: Runtime>(handle: &AppHandle<R>) {
    let new_state = {
        let facts = facts_slot().lock().expect("facts slot poisoned");
        derive(&facts)
    };

    let prev_state = {
        let mut slot = panel_slot().lock().expect("panel slot poisoned");
        if slot.as_ref() == Some(&new_state) {
            return;
        }
        let prev = slot.clone();
        *slot = Some(new_state.clone());
        prev
    };

    // 1. stdout JSONL
    let _ = Output::Panel {
        state: new_state.clone(),
    }
    .emit();

    // 2. panel webview React listener
    let _ = handle.emit_to(webview::PANEL_LABEL, EVENT_PANEL, &new_state);

    // 3. reflow — panel webview either takes its slice or collapses to 0
    webview::reflow(handle);

    // 4. post-sign-in redirect: when we transition out of the SignInToX
    //    condition (whether to Hidden or to CreateXTeam) in X-App mode,
    //    bounce the content webview to https://console.x.ai/ so we land
    //    on the canonical signed-in page even if OAuth left us in some
    //    in-between origin.
    let was_signin = matches!(
        prev_state,
        Some(PanelState::Show {
            condition: PanelCondition::SignInToX,
            ..
        })
    );
    let is_signin = matches!(
        new_state,
        PanelState::Show {
            condition: PanelCondition::SignInToX,
            ..
        }
    );
    if was_signin && !is_signin && matches!(mode::get(), Some(Mode::XApp)) {
        if let Some(content) = handle.get_webview(webview::CONTENT_LABEL) {
            if let Ok(target) = Url::parse("https://console.x.com/") {
                let _ = content.navigate(target);
            }
        }
    }
}

// ---------------------------------------------------------------------
// JWT helpers (legacy SignedInInfo extraction)
// ---------------------------------------------------------------------

/// Decode the auth token's payload into [`SignedInInfo`] if it's a
/// JWT. x.com's `auth_token` is an opaque session string, not a
/// JWT, so this will return `None` for it — and the legacy
/// `Output::SignedIn`/`SignedInPayload::info` field will stay
/// `None`. Kept as best-effort scaffolding for future modes whose
/// auth token actually carries identity claims.
fn jwt_to_info(token: &str) -> Option<SignedInInfo> {
    let payload_b64 = token.split('.').nth(1)?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    Some(SignedInInfo {
        session_id: pick_string(&claims, &["session_id", "sid"]),
        handle: pick_string(
            &claims,
            &[
                "handle",
                "preferred_username",
                "username",
                "screen_name",
                "name",
            ],
        ),
        email: pick_string(&claims, &["email"]),
        user_id: pick_string(&claims, &["sub", "user_id", "uid"]),
    })
}

fn pick_string(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}
