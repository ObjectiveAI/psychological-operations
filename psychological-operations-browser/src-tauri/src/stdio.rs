//! Tauri-side runtime for the JSON-Lines stdio protocol.
//!
//! ## Flow
//!
//! 1. At startup, [`crate::run`] creates a oneshot `mpsc` channel
//!    and stores the sender in Tauri state as [`ReadyTx`]. It hands
//!    the receiver to [`start`].
//! 2. The frontend overlay, once mounted and after its
//!    `listen("psyops:request", ...)` promise resolves, invokes
//!    [`frontend_ready`] which sends `()` on the channel.
//! 3. [`start`]'s thread blocks on `ready_rx.recv()` before opening
//!    stdin. The OS pipe already buffers anything the host wrote
//!    during startup, so no in-process buffering is needed.
//! 4. For each parsed [`Request`], [`dispatch_request`]:
//!    a. Updates [`CurrentMode`] (so a post-navigation overlay can
//!       resume URL reporting by querying [`current_mode`]).
//!    b. Creates a [`std::sync::mpsc::sync_channel`] for the ack,
//!       stashes the sender in [`PendingAck`].
//!    c. Emits the request to the window as a `psyops:request`
//!       Tauri event.
//!    d. Blocks on the receiver until the overlay calls
//!       [`stdio_respond`].
//!    e. Emits the resulting [`Output::Response`] on stdout.
//! 5. URL output is entirely frontend-driven via [`report_url`];
//!    there is no Rust-side `on_navigation` hook.
//!
//! Every byte the browser writes goes through [`Output::emit`] —
//! no `println!`, no `eprintln!`, no direct stderr from our code.

use std::io::BufRead;
use std::sync::Mutex;
use std::sync::mpsc;

use psychological_operations_browser_sdk::mode::Mode;
use psychological_operations_browser_sdk::output::Output;
use psychological_operations_browser_sdk::request::Request;
use psychological_operations_browser_sdk::response::ResponseOutcome;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Tauri event channel the browser emits stdio requests on.
/// Follows the `psyops:<topic>` naming convention.
const EVENT_REQUEST: &str = "psyops:request";

/// Tauri state — the sender half of the one-shot ready signal.
/// Taken out and consumed by the first [`frontend_ready`] call.
pub struct ReadyTx(pub Mutex<Option<mpsc::Sender<()>>>);

/// Tauri state — the sender half of the in-flight request's ack
/// channel. `None` when no request is awaiting a response.
pub struct PendingAck(pub Mutex<Option<mpsc::SyncSender<ResponseOutcome>>>);

/// Tauri state — the active session mode. Updated by
/// [`dispatch_request`] before emitting a mode-setting event so the
/// post-navigation overlay can query [`current_mode`] and resume.
pub struct CurrentMode(pub Mutex<Option<Mode>>);

/// Spawn the stdin reader thread. It waits on `ready_rx` for the
/// frontend's `frontend_ready` signal before opening stdin.
pub fn start<R: Runtime>(handle: AppHandle<R>, ready_rx: mpsc::Receiver<()>) {
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

fn dispatch_request<R: Runtime>(handle: &AppHandle<R>, req: Request) {
    // 1. Update CurrentMode for any subsequent `current_mode` query
    //    from a post-navigation overlay re-mount.
    if let Request::XApp = req {
        let mode: tauri::State<CurrentMode> = handle.state();
        *mode.0.lock().expect("mode lock poisoned") = Some(Mode::XApp);
    }

    // 2. Register a pending-ack slot before emitting so the window's
    //    stdio_respond call always finds a sender to fulfill.
    let pending: tauri::State<PendingAck> = handle.state();
    let (tx, rx) = mpsc::sync_channel::<ResponseOutcome>(1);
    *pending.0.lock().expect("pending lock poisoned") = Some(tx);

    // 3. Emit to the window. If emit itself fails, we cancel the
    //    pending slot ourselves so it doesn't leak to the next req.
    if let Err(e) = handle.emit(EVENT_REQUEST, &req) {
        pending.0.lock().expect("pending lock poisoned").take();
        let _ = Output::Response {
            result: ResponseOutcome::Err {
                error: format!("emit failed: {e}"),
            },
        }
        .emit();
        return;
    }

    // 4. Block on the ack from the window.
    let outcome = rx.recv().unwrap_or_else(|e| ResponseOutcome::Err {
        error: format!("ack channel closed: {e}"),
    });
    let _ = Output::Response { result: outcome }.emit();
}

/// Invoked once by the overlay after its `psyops:request` listener
/// is registered. Subsequent calls (e.g. after a navigation
/// re-mounts the overlay) are no-ops.
#[tauri::command]
pub fn frontend_ready(ready: tauri::State<'_, ReadyTx>) -> Result<(), String> {
    if let Some(tx) = ready.0.lock().map_err(|e| e.to_string())?.take() {
        let _ = tx.send(()); // receiver may already be gone
    }
    Ok(())
}

/// Invoked by the overlay to fulfill an in-flight request's ack.
/// Returns Err if there is no pending request — that's a benign
/// race after a post-navigation re-mount where the previous overlay
/// already acked; the caller can ignore.
#[tauri::command]
pub fn stdio_respond(
    result: ResponseOutcome,
    pending: tauri::State<'_, PendingAck>,
) -> Result<(), String> {
    let tx = pending
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or_else(|| "no pending request to ack".to_string())?;
    tx.send(result)
        .map_err(|_| "ack receiver dropped".to_string())
}

/// Invoked by the overlay on every mount. Lets the overlay resume
/// URL reporting after a navigation has re-mounted it onto a new
/// origin — if mode is `Some(XApp)`, the overlay installs its URL
/// reporter immediately.
#[tauri::command]
pub fn current_mode(mode: tauri::State<'_, CurrentMode>) -> Result<Option<Mode>, String> {
    Ok(*mode.0.lock().map_err(|e| e.to_string())?)
}

/// Invoked by the overlay for the initial URL after install and on
/// every SPA route change (`pushState` / `replaceState` /
/// `popstate` / `hashchange`). Emits [`Output::Url`].
#[tauri::command]
pub fn report_url(url: String) -> Result<(), String> {
    Output::Url { url }.emit().map_err(|e| e.to_string())
}
