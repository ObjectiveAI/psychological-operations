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
/// one recompute — safe to use as a standalone setter.
pub fn set_mode<R: Runtime>(handle: &AppHandle<R>, new_mode: Option<Mode>) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if facts.mode == new_mode {
            return;
        }
        facts.mode = new_mode;
    }
    recompute_and_publish(handle);
}

/// Set the x.com auth-token observation. Emits the legacy
/// [`Output::SignedIn`] line on every value change (for headless /
/// CLI consumers of the JSONL stream) before triggering the panel
/// recompute. If the cookies-watcher ever tracks more than one
/// cookie, switch back to a batched setter that takes all the
/// cookies together — single-cookie setters would emit
/// intermediate `PanelState`s while the second cookie was being
/// written.
pub fn set_auth_token<R: Runtime>(handle: &AppHandle<R>, token: Option<String>) {
    let token_for_emit;
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if facts.auth_token == token {
            return;
        }
        facts.auth_token = token.clone();
        token_for_emit = token;
    }

    let info = token_for_emit.as_deref().and_then(jwt_to_info);
    let _ = Output::SignedIn {
        signed_in: token_for_emit.is_some(),
        info,
    }
    .emit();

    recompute_and_publish(handle);
}

// ---------------------------------------------------------------------
// Derivation + publication
// ---------------------------------------------------------------------

/// Pure mapping from raw facts to the panel state the UI should show.
/// **The heart of the abstraction** — to add a new condition, add a
/// [`PanelCondition`] variant in the SDK and a new arm here.
///
/// Today there's only the sign-in gate; the "needs app setup"
/// condition (the equivalent of the old console.x.ai `CreateXTeam`
/// state) is a known follow-up — once we figure out whether it's
/// URL-driven, cookie-driven, or JS-pushed, it lands as a new arm
/// in this same match.
pub fn derive(facts: &Facts) -> PanelState {
    match facts.mode {
        Some(Mode::XApp) => {
            if facts.auth_token.is_none() {
                return PanelState::Show {
                    condition: PanelCondition::SignInToX,
                    message: "Sign in to X.".into(),
                };
            }
            PanelState::Hidden
        }
        _ => PanelState::Hidden,
    }
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
