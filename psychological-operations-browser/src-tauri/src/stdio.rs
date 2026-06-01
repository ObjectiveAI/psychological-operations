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

use psychological_operations_sdk::browser::output::Output;
use psychological_operations_sdk::browser::panel::PanelState;
use psychological_operations_sdk::browser::request::Request;
use psychological_operations_sdk::browser::response::ResponseOutcome;
use psychological_operations_sdk::browser::x_app_credentials::{OAuthPopup, PostCreateDialog};
use tauri::{AppHandle, Manager, Wry};

use crate::WatcherKick;
use crate::cef;
use crate::cookies_watcher;
use crate::credentials;
use crate::state;

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
    // `Shutdown` short-circuits the overlay round-trip — initiate a
    // clean tauri exit and let the runtime drain the main loop. The
    // host (CLI's `login` command) sends this after seeing
    // `AuthorizeSucceeded` / `AuthorizeFailed`.
    if matches!(req, Request::Shutdown) {
        handle.exit(0);
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

/// Save the post-create dialog HTML snapshot, re-load it through
/// the shared SDK parser, and return the parsed field count so the
/// frontend's green-state signal reflects "snapshot persisted AND
/// re-parses cleanly via the same code every other consumer uses"
/// — not just "bytes hit disk".
pub async fn process_post_create_html_inner(
    app: &AppHandle<Wry>,
    html: String,
) -> Result<u8, String> {
    let Some(user_id) = state::current_user_id() else {
        return Err("no user_id yet — cookies watcher hasn't observed twid".into());
    };

    let path = credentials::save_post_create_dialog(app, &user_id, &html).await?;
    let parsed = PostCreateDialog::load(&path)
        .await
        .map_err(|e| format!("re-parse snapshot: {e}"))?
        .unwrap_or_default();

    let _ = Output::Log {
        message: format!(
            "credentials: wrote {} ({} / 3 fields parsed)",
            path.display(),
            parsed.parsed_count(),
        ),
    }
    .emit();

    // Refresh `Facts::credentials_complete` from disk so the panel
    // transitions to Hidden the moment the snapshot lands — no
    // waiting for the next cookie kick.
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        state::recheck_credentials(&app_for_task).await;
    });
    Ok(parsed.parsed_count())
}

/// Twin of [`process_post_create_html_inner`] for the OAuth 2.0
/// popup. Same save-then-verify-via-SDK pattern, two fields
/// (`client_id`, `client_secret`).
pub async fn process_oauth_popup_html_inner(
    app: &AppHandle<Wry>,
    html: String,
) -> Result<u8, String> {
    let Some(user_id) = state::current_user_id() else {
        return Err("no user_id yet — cookies watcher hasn't observed twid".into());
    };

    let path = credentials::save_oauth_popup(app, &user_id, &html).await?;
    let parsed = OAuthPopup::load(&path)
        .await
        .map_err(|e| format!("re-parse snapshot: {e}"))?
        .unwrap_or_default();

    let _ = Output::Log {
        message: format!(
            "credentials: wrote {} ({} / 2 fields parsed)",
            path.display(),
            parsed.parsed_count(),
        ),
    }
    .emit();

    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        state::recheck_credentials(&app_for_task).await;
    });
    Ok(parsed.parsed_count())
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
