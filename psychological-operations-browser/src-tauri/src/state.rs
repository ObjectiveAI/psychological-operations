//! Process-global session state — the central fact store + derived
//! [`PanelState`] for the instruction panel.
//!
//! Watchers (cookies + URL reporting) and the stdio dispatcher (for
//! mode) contribute raw observations via [`set_mode`],
//! [`apply_cookie_facts`], [`set_current_url`],
//! [`recheck_credentials`]. Each setter updates the [`Facts`]
//! slot and calls [`recompute_and_publish`], which:
//!
//! 1. Runs the pure [`derive`] function over the new facts.
//! 2. Compares the result against the previously-published
//!    [`PanelState`]; bails if unchanged.
//! 3. Publishes the new state three places:
//!    - stdout as [`Output::Panel`]
//!    - the panel webview's Tauri-event listener (`psyops:panel`)
//!    - the X-App window reflow (panel resizes 0 ↔ [`PANEL_HEIGHT`])
//!    - the CEF content surface: post-sign-in redirect to
//!      `https://console.x.com/` on the
//!      `SignInToX → !SignInToX` transition (via
//!      [`crate::cef::navigate`]).
//!
//! Adding a new panel condition: add a [`PanelCondition`] variant in
//! the SDK + a new fact field here + a new setter + an arm in
//! [`derive`]. The panel React and reflow logic both consume only
//! the derived state — they need no changes.

use std::sync::{Mutex, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use psychological_operations_sdk::browser::mode::{self, Mode};
use psychological_operations_sdk::browser::output::{Output, SignedInInfo};
use psychological_operations_sdk::browser::panel::{PanelCondition, PanelState};
use tauri::{AppHandle, Emitter, Url, Wry};

use crate::webview;

/// Tauri event name fired on every [`PanelState`] change.
const EVENT_PANEL: &str = "psyops:panel";

/// Raw observations watchers contribute. Everything in derived
/// [`PanelState`] is a pure function of these fields + the
/// process-static [`mode::get`].
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// x.com's `auth_token` HttpOnly cookie value, if present.
    /// `None` ⇒ signed out.
    pub auth_token: Option<String>,
    /// Most recent URL the content surface reported (overlay
    /// `report_url` invoke or CEF `DisplayHandler::on_address_change`).
    /// X-App-mode-only — [`set_current_url`] is the sole setter
    /// and other modes simply never call it, so this field stays
    /// empty for them and [`derive`] doesn't have to special-case.
    pub current_url: Option<String>,
    /// X user-id parsed from the `twid` cookie by the cookies
    /// watcher. Stable per signed-in account. Used by the
    /// overlay's per-user credential-storage flow. `None` ⇒ no
    /// twid cookie yet (signed out or pre-snapshot).
    pub user_id: Option<String>,
    /// `Some(true)` iff all three X-App OAuth credentials
    /// (consumer key, secret key, bearer token) are on disk
    /// under `handles/<user_id>/` for the currently-signed-in
    /// user. `Some(false)` ⇒ at least one is missing → the
    /// panel pushes the user through the create-app flow.
    /// `None` ⇒ `user_id` isn't known yet (panel stays
    /// hidden until we can answer the question).
    ///
    /// Refreshed atomically inside [`apply_cookie_facts`] on
    /// every cookie snapshot, and on-demand by
    /// [`recheck_credentials`] after a freshly-extracted set
    /// of credentials lands on disk.
    pub credentials_complete: Option<bool>,
    /// `Some(true)` iff both OAuth 2.0 client-pair fields
    /// (`client_id`, `client_secret`) are on disk under the
    /// same `handles/<user_id>/` directory. Tracked separately
    /// from `credentials_complete` because the post-create
    /// dialog doesn't surface these — they fall out of the
    /// auth-settings popup after Save Changes, captured via
    /// `process_oauth_popup_html`. Refreshed in lock-step with
    /// `credentials_complete`.
    pub oauth_client_complete: Option<bool>,
    /// Count of *production* apps the overlay observed in the
    /// Apps list (under the `<h3>production</h3>` section).
    /// `None` ⇒ the overlay hasn't reported yet (off `/apps`,
    /// or first tick still pending). `Some(0)` is the tie-
    /// breaker that collapses the OAuth-client flow back into
    /// the create-app flow even when the first three creds are
    /// already on disk — "no production app means restart".
    /// X-App-only.
    pub production_app_count: Option<u32>,
    /// If the signed-in twid is already authorized to ANOTHER
    /// psyop on disk (i.e. `psyop/<other>/handles/<twid>/auth.json`
    /// exists), the conflicting psyop's name. `None` when no
    /// conflict, no twid is known, or the current mode isn't
    /// a psyop variant. Refreshed under the same lock as
    /// `auth_token + user_id` in [`apply_cookie_facts`].
    pub twid_conflict: Option<String>,
    /// Running count of unique tweet IDs the
    /// [`crate::psyop_read`] dedup-emitter has observed
    /// during this session. `None` ⇒ counter not rendered
    /// (pre-first-HTML or non-Read mode). Driven by
    /// [`set_tweets_read_count`].
    pub tweets_read_count: Option<u32>,
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

/// `true` iff the current twid belongs to some other psyop.
/// Cheap accessor for [`crate::psyop_read::process_html`] to
/// gate "don't dedup the wrong account's timeline into our
/// set" without taking a fact-store dependency.
pub fn twid_conflict_present() -> bool {
    facts_slot()
        .lock()
        .expect("facts slot poisoned")
        .twid_conflict
        .is_some()
}

// ---------------------------------------------------------------------
// Setters (each triggers a recompute)
// ---------------------------------------------------------------------

/// Update the most-recent-URL fact from `report_url` (or CEF's
/// `OnAddressChange`). X-App-only by convention — only the X-App
/// overlay's helpers call into it.
pub fn set_current_url(handle: &AppHandle<Wry>, url: String) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if facts.current_url.as_deref() == Some(url.as_str()) {
            return;
        }
        facts.current_url = Some(url);
    }
    recompute_and_publish(handle);
}

/// Update the production-app count the overlay observed on
/// `/apps`. X-App-only. Passing `None` clears the fact — used
/// when the overlay leaves `/apps` so a stale count can't drive
/// the wrong fallback on a different page.
///
/// The count is only consulted by `derive` when the first three
/// creds are present but the access tokens aren't:
/// `Some(0)` collapses back into the create-app flow,
/// `Some(_)` triggers `ClickProductionApp`,
/// `None` keeps the panel quiet until the overlay reports.
pub fn set_production_app_count(
    handle: &AppHandle<Wry>,
    count: Option<u32>,
) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if facts.production_app_count == count {
            return;
        }
        facts.production_app_count = count;
    }
    recompute_and_publish(handle);
}

/// Update the running "tweets read" counter the PsyopRead
/// panel surfaces. Called by [`crate::psyop_read::process_html`]
/// every time the in-memory seen set grows. PsyopRead-only by
/// convention — only the Save button's `process_read_html`
/// route calls into it.
pub fn set_tweets_read_count(handle: &AppHandle<Wry>, count: u32) {
    {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        if facts.tweets_read_count == Some(count) {
            return;
        }
        facts.tweets_read_count = Some(count);
    }
    recompute_and_publish(handle);
}

/// Re-scan the on-disk credentials store under the current
/// `user_id` and update [`Facts::credentials_complete`] +
/// [`Facts::oauth_client_complete`]. Both presence checks run
/// concurrently. Triggers a recompute only if either value changed.
///
/// Two callers:
///   - [`apply_cookie_facts`] (after every cookie snapshot, so
///     a fresh `user_id` immediately produces the right
///     answer);
///   - [`crate::stdio::process_post_create_html_inner`] (right
///     after a successful snapshot write, so the panel goes
///     `Hidden` without waiting for the next cookies kick).
pub async fn recheck_credentials(handle: &AppHandle<Wry>) {
    let uid = facts_slot()
        .lock()
        .expect("facts slot poisoned")
        .user_id
        .clone();
    let (next_first, next_access) = match uid.as_deref() {
        Some(u) => {
            let (a, b) = tokio::join!(
                crate::credentials::post_create_present(handle, u),
                crate::credentials::oauth_popup_present(handle, u),
            );
            (Some(a), Some(b))
        }
        None => (None, None),
    };
    let changed = {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        let first_changed = facts.credentials_complete != next_first;
        let access_changed = facts.oauth_client_complete != next_access;
        if first_changed {
            facts.credentials_complete = next_first;
        }
        if access_changed {
            facts.oauth_client_complete = next_access;
        }
        first_changed || access_changed
    };
    if changed {
        recompute_and_publish(handle);
    }
}

/// Atomically update every cookie-sourced fact from a single
/// observation. Both `auth_token` and `user_id` land under a
/// single lock so no intermediate `PanelState` leaks between them.
///
/// On every `auth_token` value change:
///   1. Emits [`Output::SignedIn`].
///   2. If the new value is `Some(_)` (signed in), bounces the
///      CEF content surface to the mode's canonical home — X-App
///      → `console.x.com/`, Psyop → `x.com/`. This lands the user
///      on a stable page even if OAuth left them on an
///      in-between origin.
///
/// Then triggers the panel recompute.
pub async fn apply_cookie_facts(
    handle: &AppHandle<Wry>,
    auth_token: Option<String>,
    user_id: Option<String>,
) {
    // Concurrent: presence-check both snapshot files + (if
    // applicable) the cross-psyop conflict walk. All three are
    // disk-bound and independent — `tokio::join!` runs them in
    // parallel. Mode is locked at startup so it's safe to read
    // outside any lock.
    let conflict_psyop = current_psyop_name();
    let (creds_complete, oauth_complete, twid_conflict) = match user_id.as_deref() {
        Some(uid) => {
            let post_create_fut = crate::credentials::post_create_present(handle, uid);
            let oauth_fut = crate::credentials::oauth_popup_present(handle, uid);
            let conflict_fut = async {
                match (auth_token.as_ref(), conflict_psyop.as_deref()) {
                    (Some(_), Some(name)) => {
                        crate::authorize::find_other_psyop_owning_twid(
                            handle, name, uid,
                        )
                        .await
                    }
                    _ => None,
                }
            };
            let (a, b, c) = tokio::join!(post_create_fut, oauth_fut, conflict_fut);
            (Some(a), Some(b), c)
        }
        None => (None, None, None),
    };

    let token_changed_to: Option<Option<String>> = {
        let mut facts = facts_slot().lock().expect("facts slot poisoned");
        let token_changed = facts.auth_token != auth_token;
        facts.auth_token = auth_token.clone();
        facts.user_id = user_id;
        facts.credentials_complete = creds_complete;
        facts.oauth_client_complete = oauth_complete;
        facts.twid_conflict = twid_conflict;
        if token_changed { Some(auth_token) } else { None }
    };

    if let Some(new_token) = token_changed_to {
        let info = new_token.as_deref().and_then(jwt_to_info);
        let _ = Output::SignedIn {
            signed_in: new_token.is_some(),
            info,
        }
        .emit();

        // Just signed in? Redirect to the mode's home page. No-op
        // on signed-in → signed-out (x.com handles its own logout
        // navigation; we don't pin them to a login URL).
        if new_token.is_some() {
            if let Some(url) = home_url_for_current_mode() {
                crate::cef::navigate(url);
            }
        } else {
            // Signed out — clear the psyop-authorize one-shot
            // so a future sign-in (potentially with a different
            // twid) re-engages the OAuth flow.
            crate::authorize::clear_in_flight_on_signout();
        }
    }

    recompute_and_publish(handle);
}

/// Canonical home URL the post-sign-in redirect bounces to per
/// mode. Matches [`crate::webview`]'s start-URL choice for each
/// mode at browser-creation time.
fn home_url_for_current_mode() -> Option<&'static str> {
    match mode::get()? {
        Mode::XApp => Some("https://console.x.com/"),
        Mode::PsyopRead { .. }
        | Mode::PsyopAuthorize { .. }
        | Mode::AgentAuthorize { .. }
        | Mode::PsyopBrowser { .. }
        | Mode::AgentBrowser { .. } => Some("https://x.com/"),
    }
}

/// The current psyop's name, if any. Both psyop variants share
/// the same on-disk dir and therefore the same conflict domain,
/// so the variant tag doesn't matter — only the name does.
fn current_psyop_name() -> Option<String> {
    match mode::get()? {
        Mode::PsyopRead { name } | Mode::PsyopAuthorize { name } => Some(name),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Derivation + publication
// ---------------------------------------------------------------------

/// Pure mapping from raw facts (+ the locked-at-startup mode)
/// to the panel state the UI should show. **The heart of the
/// abstraction** — to add a new condition, add a
/// [`PanelCondition`] variant in the SDK and a new arm here.
pub fn derive(facts: &Facts) -> PanelState {
    match mode::get() {
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
            // Two-layered decision keyed on
            // (creds_complete, oauth_client_complete):
            //
            //   creds_complete drives the first triple. If it's
            //   missing, we push the user through the create-
            //   app flow regardless of access_tokens. If it's
            //   present, access_tokens picks up — push them to
            //   their existing production app to capture the
            //   final pair, or fall back to create-app if no
            //   production app exists (a deleted app means
            //   the on-disk first triple is stale anyway).
            //
            //   `None` for either fact = "we don't know yet"
            //   → stay quiet so we don't flash a wrong
            //   message between mount and the first cookie
            //   snapshot / apps-page read.
            // Three URL bands:
            //   on_list  — strictly /apps[/]?, the apps list
            //              page. Count-driven conditions
            //              (ClickCreateApp / ClickProductionApp)
            //              only ever fire here.
            //   on_area  — anywhere under /apps/* including
            //              individual app pages. While in the
            //              area but not on the list (i.e. inside
            //              an app), the panel hides — the
            //              access-token capture flow that fills
            //              that space lands in a future plan.
            //   else     — outside the Apps area entirely, push
            //              them in via ClickAppsTab.
            let on_list = is_apps_list(url);
            let on_app = is_app_page(url);
            let on_auth = is_auth_settings(url);
            let on_area = is_apps_tab(url);
            match (facts.credentials_complete, facts.oauth_client_complete) {
                // We don't know enough yet (cookies snapshot
                // hasn't landed). Show Loading rather than
                // collapsing the panel — same intent as
                // "thinking…" so the user doesn't think the UI
                // is broken.
                (None, _) => PanelState::Show {
                    condition: PanelCondition::Loading,
                    message: String::new(),
                },
                (Some(false), _) => {
                    if on_list {
                        PanelState::Show {
                            condition: PanelCondition::ClickCreateApp,
                            message: "Click Create App.".into(),
                        }
                    } else if on_area {
                        // Inside an app — quiet until the future
                        // per-app flow has something to say.
                        PanelState::Hidden
                    } else {
                        PanelState::Show {
                            condition: PanelCondition::ClickAppsTab,
                            message: "Click the Apps tab.".into(),
                        }
                    }
                }
                (Some(true), Some(true)) => PanelState::Hidden,
                (Some(true), Some(false) | None) => {
                    if !on_area {
                        PanelState::Show {
                            condition: PanelCondition::ClickAppsTab,
                            message: "Click the Apps tab.".into(),
                        }
                    } else if on_list {
                        match facts.production_app_count {
                            Some(0) => PanelState::Show {
                                condition: PanelCondition::ClickCreateApp,
                                message: "Click Create App.".into(),
                            },
                            Some(_) => PanelState::Show {
                                condition: PanelCondition::ClickProductionApp,
                                message: "Click your app.".into(),
                            },
                            // Count still debouncing — same
                            // Loading affordance as the
                            // creds-unknown case above.
                            None => PanelState::Show {
                                condition: PanelCondition::Loading,
                                message: String::new(),
                            },
                        }
                    } else if on_app {
                        // Inside an app's overview page — push
                        // them to Settings, where the
                        // access-token pair gets generated.
                        PanelState::Show {
                            condition: PanelCondition::ClickSettings,
                            message: "Click Settings.".into(),
                        }
                    } else if on_auth {
                        // Auth-settings page — wizard-style
                        // multi-badge overlay walks the user
                        // through configuring scopes, type,
                        // website URL, callback URI, and Save.
                        PanelState::Show {
                            condition: PanelCondition::ConfigureAuthSettings,
                            message: "Configure auth settings.".into(),
                        }
                    } else {
                        // Other sub-routes — quiet until the
                        // per-tab capture flow lands.
                        PanelState::Hidden
                    }
                }
            }
        }
        Some(Mode::PsyopRead { .. }) => {
            // PsyopRead: nag to sign in if not signed in;
            // surface the twid-conflict guard ahead of any
            // counter so the user fixes the wrong-account
            // problem before they assume tweet IDs are
            // being captured under the right persona;
            // otherwise show the running tweet counter the
            // overlay+`psyop_read` module drives.
            if facts.auth_token.is_none() {
                PanelState::Show {
                    condition: PanelCondition::SignInToX,
                    message: "Sign in to X.".into(),
                }
            } else if let Some(other) = &facts.twid_conflict {
                PanelState::Show {
                    condition: PanelCondition::PsyopAccountInUse,
                    message: format!(
                        "Sign out. This account is already in use by PsyOp {other}."
                    ),
                }
            } else {
                PanelState::Show {
                    condition: PanelCondition::TweetsRead,
                    message: format!(
                        "Tweets read: {}",
                        facts.tweets_read_count.unwrap_or(0)
                    ),
                }
            }
        }
        Some(Mode::PsyopAuthorize { .. }) => {
            // PsyopAuthorize: Rust auto-navigates to X's
            // OAuth authorize page once the persona signs
            // in, and X's own page is the affordance — no
            // helper widget on top. The only panel surface
            // is the sign-in nag and the twid-conflict
            // guard.
            if facts.auth_token.is_none() {
                PanelState::Show {
                    condition: PanelCondition::SignInToX,
                    message: "Sign in to X.".into(),
                }
            } else if let Some(other) = &facts.twid_conflict {
                PanelState::Show {
                    condition: PanelCondition::PsyopAccountInUse,
                    message: format!(
                        "Sign out. This account is already in use by PsyOp {other}."
                    ),
                }
            } else {
                PanelState::Hidden
            }
        }
        Some(Mode::AgentAuthorize { .. }) => {
            // AgentAuthorize: same shape as PsyopAuthorize —
            // Rust auto-navigates to X's OAuth authorize page
            // once the persona signs in. No twid-conflict
            // guard for agents (per design: multiple agents
            // can share the same X account).
            if facts.auth_token.is_none() {
                PanelState::Show {
                    condition: PanelCondition::SignInToX,
                    message: "Sign in to X.".into(),
                }
            } else {
                PanelState::Hidden
            }
        }
        Some(Mode::PsyopBrowser { .. }) | Some(Mode::AgentBrowser { .. }) => {
            // Browser modes: just open the persona's browser at
            // x.com and let the operator do whatever. Only nag
            // for sign-in; otherwise hide the panel. No twid-
            // conflict guard, no read-scrape counter, no OAuth
            // flow. The overlay JS is gated out in cef.rs so
            // nothing custom runs in the page either.
            if facts.auth_token.is_none() {
                PanelState::Show {
                    condition: PanelCondition::SignInToX,
                    message: "Sign in to X.".into(),
                }
            } else {
                PanelState::Hidden
            }
        }
        None => PanelState::Hidden,
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

/// Stricter sibling of [`is_apps_tab`]: only the apps LIST page
/// itself (`/accounts/<id>/apps[/]?`), not any individual-app
/// sub-route. `derive` uses it to gate count-driven panel
/// conditions — those only make sense when the list (and its
/// "Create App" button + production section) is the page being
/// viewed.
fn is_apps_list(url: Option<&str>) -> bool {
    let Some(url) = url else { return false };
    let Ok(parsed) = Url::parse(url) else { return false };
    if parsed.host_str() != Some("console.x.com") {
        return false;
    }
    let mut segs = parsed.path().split('/').filter(|s| !s.is_empty());
    matches!(segs.next(), Some("accounts"))
        && segs.next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
        && matches!(segs.next(), Some("apps"))
        && segs.next().is_none()
}

/// Strictly the individual *app's overview* page —
/// `/accounts/<id>/apps/<numeric-app-id>[/]?` with no further
/// segments. App ids in the URL are numeric; the literal token
/// `settings` is the auth-settings page and is matched
/// separately by [`is_auth_settings`].
fn is_app_page(url: Option<&str>) -> bool {
    let Some(url) = url else { return false };
    let Ok(parsed) = Url::parse(url) else { return false };
    if parsed.host_str() != Some("console.x.com") {
        return false;
    }
    let mut segs = parsed.path().split('/').filter(|s| !s.is_empty());
    matches!(segs.next(), Some("accounts"))
        && segs.next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
        && matches!(segs.next(), Some("apps"))
        && segs.next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
        && segs.next().is_none()
}

/// Strict match for the dev console's auth-settings page —
/// `/accounts/<id>/apps/settings[/]?` (the app id lives in
/// `?appId=…` rather than the path, so we don't read it
/// here). Where `ConfigureAuthSettings` fires.
fn is_auth_settings(url: Option<&str>) -> bool {
    let Some(url) = url else { return false };
    let Ok(parsed) = Url::parse(url) else { return false };
    if parsed.host_str() != Some("console.x.com") {
        return false;
    }
    let mut segs = parsed.path().split('/').filter(|s| !s.is_empty());
    matches!(segs.next(), Some("accounts"))
        && segs.next().is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
        && matches!(segs.next(), Some("apps"))
        && matches!(segs.next(), Some("settings"))
        && segs.next().is_none()
}

/// Re-run [`derive`] on the current facts; if the result differs from
/// the last-published [`PanelState`], publish it everywhere that cares.
///
/// Post-sign-in redirect lives in [`apply_cookie_facts`] — it's
/// driven by the cookie change itself, not by a panel transition
/// (psyop mode has no `SignInToX` panel condition to transition
/// out of, so a panel-gated trigger wouldn't catch it).
pub fn recompute_and_publish(handle: &AppHandle<Wry>) {
    let new_state = {
        let facts = facts_slot().lock().expect("facts slot poisoned");
        derive(&facts)
    };

    {
        let mut slot = panel_slot().lock().expect("panel slot poisoned");
        if slot.as_ref() == Some(&new_state) {
            return;
        }
        *slot = Some(new_state.clone());
    }

    // 1. stdout JSONL
    let _ = Output::Panel {
        state: new_state.clone(),
    }
    .emit();

    // 1b. X-App setup terminator. Once per process, in `Mode::XApp`
    //     only, when the panel lands on `Hidden` (i.e. both
    //     `credentials_complete` and `oauth_client_complete` are
    //     `Some(true)`). Read by the CLI's `x_app setup` to know
    //     when to send `Request::Shutdown`.
    if matches!(
        psychological_operations_sdk::browser::mode::get(),
        Some(psychological_operations_sdk::browser::mode::Mode::XApp),
    ) && matches!(new_state, PanelState::Hidden)
    {
        static X_APP_TERMINATOR_FIRED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if X_APP_TERMINATOR_FIRED.set(()).is_ok() {
            let _ = Output::XAppSetupSucceeded.emit();
        }
    }

    // 2. panel webview React listener
    let _ = handle.emit_to(webview::PANEL_LABEL, EVENT_PANEL, &new_state);

    // 3. reflow — panel webview either takes its slice or collapses to 0
    webview::reflow(handle);

    // 4. mirror to the content overlay so per-pointer modules
    //    (apps-tab, future create-app) can gate visibility on the
    //    same fact the panel uses. Fire-and-forget; the JS setter
    //    is undefined until the overlay mounts, and the
    //    `&&`-guard makes that case a clean no-op.
    let payload = serde_json::to_string(&new_state).unwrap_or_else(|_| "null".into());
    crate::cef::execute_overlay_js(format!(
        "window.__psyops_set_panel && window.__psyops_set_panel({payload});"
    ));
}

// ---------------------------------------------------------------------
// JWT decoder for the `Output::SignedIn.info` field
// ---------------------------------------------------------------------

/// Decode the auth token's payload into [`SignedInInfo`] if it's a
/// JWT. x.com's `auth_token` is an opaque session string (not a
/// JWT) so this returns `None` and `Output::SignedIn.info` stays
/// `None` in X-App mode. Kept as best-effort scaffolding for future
/// modes whose auth token carries identity claims.
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
