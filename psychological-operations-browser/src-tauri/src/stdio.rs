//! Tauri-side runtime for the JSON-Lines stdio protocol.
//!
//! ## Flow
//!
//! 1. At startup, [`crate::run`] creates a oneshot `mpsc` channel
//!    and stores the sender in Tauri state as [`ReadyTx`]. It hands
//!    the receiver to [`start`].
//! 2. The frontend overlay, once mounted, registers
//!    `window.__psyops.push` and then invokes [`frontend_ready`]
//!    (via the `psyops://invoke/frontend_ready` scheme endpoint),
//!    which sends `()` on the channel.
//! 3. [`start`]'s thread blocks on `ready_rx.recv()` before opening
//!    stdin. The OS pipe already buffers anything the host wrote
//!    during startup, so no in-process buffering is needed.
//! 4. For each parsed [`Request`], [`dispatch_request`]:
//!    a. For mode-setting variants (currently only [`Request::XApp`]),
//!       updates the SDK's process-global mode slot so subsequent
//!       output lines carry `"mode":{"type":"x_app"}`, then
//!       (re)starts the cookies watcher.
//!    b. Creates a [`std::sync::mpsc::sync_channel`] for the ack,
//!       stashes the sender in [`PendingAck`].
//!    c. Pushes the request to the overlay by calling
//!       `cef::execute_overlay_js("window.__psyops.push(<json>)")`.
//!       The overlay handles it and calls
//!       `psyops://invoke/stdio_respond` with the result.
//!    d. Blocks on the receiver until the overlay's stdio_respond
//!       lands.
//!    e. Emits the resulting [`Output::Response`] on stdout.
//! 5. URL output is overlay-driven via [`report_url`] AND CEF's
//!    `DisplayHandler::on_address_change` (Phase 4). Both fan
//!    in to [`state::set_current_url`] and the cookies kick.
//! 6. Sign-in detection is Rust-side via [`crate::cookies_watcher`],
//!    reading CEF's `CookieManager` on every kick.
//!
//! Every byte the browser writes goes through [`Output::emit`] —
//! no `println!`, no `eprintln!`, no direct stderr from our code.
//!
//! ## Two transports, one set of commands
//!
//! Each command body is factored into a plain `*_inner` Rust
//! function that BOTH the Tauri `#[command]` wrappers AND the
//! `psyops://invoke/<cmd>` scheme dispatcher
//! ([`crate::cef_scheme`]) call. The panel webview (Tauri/WebView2)
//! reaches the inner fns through the Tauri wrappers; the CEF
//! content overlay reaches them through the scheme dispatcher.

use std::io::BufRead;
use std::sync::Mutex;
use std::sync::mpsc;

use psychological_operations_browser_sdk::credentials::XAppCredentialField;
use psychological_operations_browser_sdk::mode::{self, Mode};
use psychological_operations_browser_sdk::output::Output;
use psychological_operations_browser_sdk::panel::PanelState;
use psychological_operations_browser_sdk::request::Request;
use psychological_operations_browser_sdk::response::{Response, ResponseOutcome};
use tauri::{AppHandle, Manager, Wry};

use crate::WatcherKick;
use crate::cef;
use crate::cookies_watcher;
use crate::credentials;
use crate::oauth_popup;
use crate::post_create_dialog;
use crate::state;
use crate::webview;

/// Tauri state — the sender half of the one-shot ready signal.
/// Taken out and consumed by the first [`frontend_ready`] call.
pub struct ReadyTx(pub Mutex<Option<mpsc::Sender<()>>>);

/// Tauri state — the sender half of the in-flight request's ack
/// channel. `None` when no request is awaiting a response.
pub struct PendingAck(pub Mutex<Option<mpsc::SyncSender<ResponseOutcome>>>);

/// Tauri state — the active cookies watcher's handle, if any.
/// Dropping the previous handle when assigning a new one tears down
/// its reader task.
pub struct CookiesWatcherSlot(pub Mutex<Option<cookies_watcher::Handle>>);

/// Spawn the stdin reader thread. It waits on `ready_rx` for the
/// frontend's `frontend_ready` signal before opening stdin.
pub fn start(handle: AppHandle<Wry>, ready_rx: mpsc::Receiver<()>) {
    std::thread::spawn(move || {
        if ready_rx.recv().is_err() {
            // ReadyTx dropped without firing — app is tearing down.
            return;
        }
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req: Request = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let _ = Output::Log {
                        message: format!("stdio: dropping unparseable line: {e}"),
                    }
                    .emit();
                    continue;
                }
            };
            dispatch_request(&handle, req);
        }
    });
}

fn dispatch_request(handle: &AppHandle<Wry>, req: Request) {
    // Mode-switch requests are handled entirely on the Rust side
    // — they tear down + reopen CEF; the overlay isn't involved
    // in the dispatch. Stdin reading naturally pauses for the
    // duration because we block synchronously below.
    if let Some(new_mode) = mode_of_request(&req) {
        switch_mode(handle, new_mode);
        let _ = Output::Response {
            result: ResponseOutcome::Ok { response: Response::Ack },
        }
        .emit();
        return;
    }

    // 1. Register a pending-ack slot before pushing so the overlay's
    //    stdio_respond invoke always finds a sender to fulfill.
    let pending: tauri::State<PendingAck> = handle.state();
    let (tx, rx) = mpsc::sync_channel::<ResponseOutcome>(1);
    *pending.0.lock().expect("pending lock poisoned") = Some(tx);

    // 2. Push the request to the overlay via Frame::execute_javascript.
    //    The overlay registered `window.__psyops.push(req)` at mount.
    match serde_json::to_string(&req) {
        Ok(json) => cef::execute_overlay_js(format!(
            "window.__psyops && window.__psyops.push({json})"
        )),
        Err(e) => {
            pending.0.lock().expect("pending lock poisoned").take();
            let _ = Output::Response {
                result: ResponseOutcome::Err {
                    error: format!("serialize request failed: {e}"),
                },
            }
            .emit();
            return;
        }
    }

    // 3. Block on the ack from the overlay.
    let outcome = rx.recv().unwrap_or_else(|e| ResponseOutcome::Err {
        error: format!("ack channel closed: {e}"),
    });
    let _ = Output::Response { result: outcome }.emit();
}

/// Map a [`Request`] to the mode it switches to, or `None` if
/// it's not a mode-switch request.
fn mode_of_request(req: &Request) -> Option<Mode> {
    match req {
        Request::XApp => Some(Mode::XApp),
        Request::PsyopRead { name } => Some(Mode::PsyopRead { name: name.clone() }),
        Request::PsyopAuthorize { name } => Some(Mode::PsyopAuthorize { name: name.clone() }),
        _ => None,
    }
}

/// Synchronously switch the active CEF browser to `new_mode`.
/// Blocks the caller (the stdin reader thread) until the new
/// browser's overlay has invoked `frontend_ready`, so subsequent
/// stdin lines aren't dispatched against a half-built browser.
///
/// If `new_mode` already matches the current mode, this is a
/// no-op (Tauri stays, CEF stays).
fn switch_mode(handle: &AppHandle<Wry>, new_mode: Mode) {
    if mode::get().as_ref() == Some(&new_mode) {
        return;
    }

    // 1. Drop the current cookies watcher — the new mode's
    //    RequestContext has a fresh cookie store.
    let watcher_slot: tauri::State<CookiesWatcherSlot> = handle.state();
    *watcher_slot.0.lock().expect("watcher slot poisoned") = None;

    // 2. Flip the SDK mode static + state-facts mode in lockstep.
    //    EVERY subsequent stdout line carries the new mode.
    mode::set(Some(new_mode.clone()));
    state::set_mode(handle, Some(new_mode.clone()));

    // 3. Replace the ReadyTx slot with a fresh oneshot so we can
    //    wait for the NEW overlay's frontend_ready call (the old
    //    sender was already consumed at process startup).
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let ready_state: tauri::State<ReadyTx> = handle.state();
    *ready_state.0.lock().expect("ready slot poisoned") = Some(ready_tx);

    // 4. Tear down + recreate CEF. recreate_cef_content blocks on
    //    `on_before_close` internally.
    webview::recreate_cef_content(handle, &new_mode);

    // 5. Wait for the new overlay to call frontend_ready (loaded
    //    page + JS bundle + push handler registered + invoke went
    //    out + Rust handled it + sender fired). 30s ceiling so a
    //    page that never loads doesn't deadlock the stdin reader.
    let _ = ready_rx.recv_timeout(std::time::Duration::from_secs(30));

    // 6. Start a fresh cookies watcher scoped to the new mode.
    let data_dir = webview::mode_data_dir(handle, &new_mode);
    *watcher_slot.0.lock().expect("watcher slot poisoned") =
        cookies_watcher::start(handle.clone(), &new_mode, &data_dir);
}

// ---------------------------------------------------------------------
// Inner command functions (transport-agnostic) + Tauri wrappers
// ---------------------------------------------------------------------
//
// Each "_inner" fn is the actual command body. Two transports reach
// it: Tauri's `#[command]` macro (used by the panel webview) and
// the `psyops://invoke/<cmd>` scheme dispatcher in
// [`crate::cef_scheme`] (used by the CEF content overlay).

pub fn frontend_ready_inner(app: &AppHandle<Wry>) -> Result<(), String> {
    let ready: tauri::State<ReadyTx> = app.state();
    if let Some(tx) = ready.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = tx.send(()); // receiver may already be gone
    }
    Ok(())
}

pub fn stdio_respond_inner(
    app: &AppHandle<Wry>,
    result: ResponseOutcome,
) -> Result<(), String> {
    let pending: tauri::State<PendingAck> = app.state();
    let tx = pending
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or_else(|| "no pending request to ack".to_string())?;
    tx.send(result)
        .map_err(|_| "ack receiver dropped".to_string())
}

pub fn current_mode_inner() -> Option<Mode> {
    mode::get()
}

pub fn current_user_id_inner() -> Option<String> {
    state::current_user_id()
}

pub fn current_panel_inner() -> Option<PanelState> {
    state::current_panel()
}

pub fn report_url_inner(app: &AppHandle<Wry>, url: String) -> Result<(), String> {
    Output::Url { url: url.clone() }
        .emit()
        .map_err(|e| e.to_string())?;
    app.state::<WatcherKick>().0.notify_one();
    // Spawn so this returns immediately — `set_current_url` →
    // `recompute_and_publish` issues main-thread dispatches
    // (emit_to, set_size, set_position) that would deadlock when
    // called synchronously from inside a Tauri command handler.
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        state::set_current_url(&app_for_task, url);
    });
    Ok(())
}

pub fn set_production_app_count_inner(
    app: &AppHandle<Wry>,
    count: Option<u32>,
) -> Result<(), String> {
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        state::set_production_app_count(&app_for_task, count);
    });
    Ok(())
}

pub fn process_post_create_html_inner(
    app: &AppHandle<Wry>,
    html: String,
) -> Result<u8, String> {
    // Always snapshot — even if parse misses, we want the HTML on
    // disk so we can refine selectors offline.
    if let Err(e) = post_create_dialog::save_snapshot(app, &html) {
        let _ = Output::Log {
            message: format!("post_create_dialog: snapshot write failed: {e}"),
        }
        .emit();
    }

    let Some(user_id) = state::current_user_id() else {
        return Err("no user_id yet — cookies watcher hasn't observed twid".into());
    };

    let extracted = post_create_dialog::extract(&html);
    let mut stored: u8 = 0;
    for (field, value) in [
        (XAppCredentialField::ConsumerKey, &extracted.consumer_key),
        (XAppCredentialField::SecretKey, &extracted.secret_key),
        (XAppCredentialField::BearerToken, &extracted.bearer_token),
    ] {
        let Some(v) = value else { continue };
        match credentials::store_one(app, &user_id, field, v) {
            Ok(path) => {
                stored += 1;
                let _ = Output::Log {
                    message: format!("credentials: wrote {}", path.display()),
                }
                .emit();
            }
            Err(e) => {
                let _ = Output::Log {
                    message: format!("credentials: write failed: {e}"),
                }
                .emit();
            }
        }
    }
    // Refresh `Facts::credentials_complete` from disk so the
    // panel transitions to Hidden the moment the third file
    // lands — no waiting for the next cookie kick.
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        state::recheck_credentials(&app_for_task);
    });
    Ok(stored)
}

/// Twin of [`process_post_create_html_inner`] for the OAuth 2.0
/// popup that fires after the user clicks Save Changes on the
/// auth-settings page. Two fields here (`client_id` +
/// `client_secret`) instead of three. Calls
/// [`state::recheck_credentials`] at the end so
/// `Facts::oauth_client_complete` flips the moment both files
/// land — same self-healing pattern as
/// `process_post_create_html_inner`.
pub fn process_oauth_popup_html_inner(
    app: &AppHandle<Wry>,
    html: String,
) -> Result<u8, String> {
    if let Err(e) = oauth_popup::save_snapshot(app, &html) {
        let _ = Output::Log {
            message: format!("oauth_popup: snapshot write failed: {e}"),
        }
        .emit();
    }

    let Some(user_id) = state::current_user_id() else {
        return Err("no user_id yet — cookies watcher hasn't observed twid".into());
    };

    let extracted = oauth_popup::extract(&html);
    let mut stored: u8 = 0;
    for (field, value) in [
        (XAppCredentialField::ClientId, &extracted.client_id),
        (XAppCredentialField::ClientSecret, &extracted.client_secret),
    ] {
        let Some(v) = value else { continue };
        match credentials::store_one(app, &user_id, field, v) {
            Ok(path) => {
                stored += 1;
                let _ = Output::Log {
                    message: format!("credentials: wrote {}", path.display()),
                }
                .emit();
            }
            Err(e) => {
                let _ = Output::Log {
                    message: format!("credentials: write failed: {e}"),
                }
                .emit();
            }
        }
    }
    // Refresh `Facts::oauth_client_complete` from disk so the
    // panel transitions to Hidden the moment both files land —
    // no waiting for the next cookies kick.
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        state::recheck_credentials(&app_for_task);
    });
    Ok(stored)
}

pub fn store_x_app_credential_inner(
    app: &AppHandle<Wry>,
    handle: String,
    field: XAppCredentialField,
    value: String,
) -> Result<(), String> {
    let path = credentials::store_one(app, &handle, field, &value)?;
    let _ = Output::Log {
        message: format!("credentials: wrote {}", path.display()),
    }
    .emit();
    Ok(())
}

// ---------------------------------------------------------------------
// Tauri #[command] wrapper — only `current_panel` survives.
// All other commands are reachable solely from the CEF content
// overlay via the `psyops://` scheme, which calls the `*_inner` fns
// directly. The panel webview (local tauri://) keeps using Tauri's
// IPC for `current_panel` because it lives inside Tauri's webview
// runtime and can.
// ---------------------------------------------------------------------

#[tauri::command]
pub fn current_panel() -> Result<Option<PanelState>, String> {
    Ok(current_panel_inner())
}
