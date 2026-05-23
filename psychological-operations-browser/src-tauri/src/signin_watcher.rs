//! Filesystem-driven sign-in watcher (fully async).
//!
//! Per the user's spec: "rust will listen to changes to this
//! specific cookie. cookies may change that are not the one we care
//! about — we don't need to check anything unless that particular
//! cookie changes." We use the `notify` crate to watch the WebView2
//! cookie SQLite store at `<data-dir>/EBWebView/Default/Network/`.
//! Each filesystem event triggers a re-read of cookies for the
//! mode's auth URL; if the auth cookie's presence-or-value changed,
//! we decode its JWT payload, extract identity claims, and emit
//! [`Output::SignedIn`]. Same-value writes are coalesced — no
//! emission unless the cookie we care about actually flipped.
//!
//! Everything runs on `tauri::async_runtime` (tokio under the hood)
//! — the filesystem channel is `tokio::sync::mpsc`, the stop signal
//! is `tokio::sync::Notify`, and the synchronous Tauri
//! `cookies_for_url` call (which dispatches through the main
//! thread) is parked behind `spawn_blocking` so the async runtime
//! never blocks on it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use psychological_operations_browser_sdk::mode::Mode;
use psychological_operations_browser_sdk::output::{Output, SignedInInfo};
use tauri::async_runtime::{JoinHandle, spawn, spawn_blocking};
use tauri::{Runtime, Url, WebviewWindow};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// Handle to a running watcher. Dropping it cancels the watcher
/// (notify drops + stop signal fires, task exits at next yield).
pub struct Handle {
    _watcher: RecommendedWatcher,
    stop: Arc<Notify>,
    _task: JoinHandle<()>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

/// Start the sign-in watcher for the given mode. Returns `None`
/// for modes we haven't wired up yet (currently only [`Mode::XApp`]
/// is supported).
///
/// Performs an initial cookie read synchronously on the calling
/// thread (typically the stdio reader thread, which has free
/// access to the main thread before any JS handler runs) and
/// emits the baseline [`Output::SignedIn`] before spawning the
/// async filesystem-watching task.
pub fn start<R: Runtime>(
    window: WebviewWindow<R>,
    mode: &Mode,
    data_dir: &Path,
) -> Option<Handle> {
    let (auth_url, cookie_name): (Url, &str) = match mode {
        Mode::XApp => (Url::parse("https://console.x.ai/").ok()?, "sso"),
        Mode::Psyop { .. } => {
            let _ = Output::Log {
                message: "signin_watcher: Psyop mode not yet wired".into(),
            }
            .emit();
            return None;
        }
    };
    let cookie_name = cookie_name.to_string();

    let cookie_store_dir = data_dir.join("EBWebView").join("Default").join("Network");
    let _ = std::fs::create_dir_all(&cookie_store_dir);

    // Initial sync read — at this point the JS event handler hasn't
    // started yet (dispatch_request hasn't emitted psyops:request),
    // so the main thread is free to service cookies_for_url.
    let initial_token = read_cookie_sync(&window, auth_url.clone(), &cookie_name);
    emit_signed_in(&initial_token);

    // notify's recommended_watcher fires its callback from its own
    // internal thread. Bridge to a tokio mpsc — UnboundedSender::send
    // is sync, so safe to call from the notify thread.
    let (fs_tx, fs_rx) = unbounded_channel::<Event>();
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            let _ = fs_tx.send(ev);
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            let _ = Output::Log {
                message: format!("signin_watcher: failed to create watcher: {e}"),
            }
            .emit();
            return None;
        }
    };
    // The Cookies SQLite file is rewritten via journal / WAL side
    // files; watch the parent dir non-recursively so we catch all
    // edge writes.
    if let Err(e) = watcher.watch(&cookie_store_dir, RecursiveMode::NonRecursive) {
        let _ = Output::Log {
            message: format!(
                "signin_watcher: failed to watch {}: {e}",
                cookie_store_dir.display()
            ),
        }
        .emit();
        return None;
    }

    let stop = Arc::new(Notify::new());
    let stop_for_task = stop.clone();
    let task = spawn(async move {
        run_watcher(window, auth_url, cookie_name, initial_token, fs_rx, stop_for_task).await;
    });

    Some(Handle {
        _watcher: watcher,
        stop,
        _task: task,
    })
}

/// Synchronous cookie read for the initial baseline. Returns the
/// auth-cookie value if present, `None` otherwise.
fn read_cookie_sync<R: Runtime>(
    window: &WebviewWindow<R>,
    auth_url: Url,
    cookie_name: &str,
) -> Option<String> {
    match window.cookies_for_url(auth_url) {
        Ok(cookies) => cookies
            .into_iter()
            .find(|c| c.name() == cookie_name)
            .map(|c| c.value().to_string()),
        Err(e) => {
            let _ = Output::Log {
                message: format!("signin_watcher: initial cookies_for_url err: {e}"),
            }
            .emit();
            None
        }
    }
}

fn emit_signed_in(token: &Option<String>) {
    let (signed_in, info) = match token {
        Some(t) => (true, jwt_to_info(t)),
        None => (false, None),
    };
    let _ = Output::SignedIn { signed_in, info }.emit();
}

async fn run_watcher<R: Runtime>(
    window: WebviewWindow<R>,
    auth_url: Url,
    cookie_name: String,
    initial_token: Option<String>,
    mut fs_rx: UnboundedReceiver<Event>,
    stop: Arc<Notify>,
) {
    // Initial state has been emitted by `start` before spawning;
    // seed `last` with it so we only emit on actual changes.
    let mut last: Option<String> = initial_token;

    loop {
        tokio::select! {
            _ = stop.notified() => break,
            ev = fs_rx.recv() => {
                match ev {
                    Some(e) if event_is_write(&e) => {
                        // Coalesce burst writes (sqlite main file +
                        // -journal + -wal all flip per single
                        // commit). Sleep briefly then drain anything
                        // that arrived during the sleep.
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        while fs_rx.try_recv().is_ok() {}
                        // `try_read_cookie` returns `None` on read
                        // failure (timeout / dispatcher error); we
                        // simply skip those rather than mistaking
                        // them for a logout.
                        if let Some(state) = try_read_cookie(
                            window.clone(),
                            auth_url.clone(),
                            cookie_name.clone(),
                        )
                        .await
                        {
                            maybe_emit(&mut last, state);
                        }
                    }
                    Some(_) => {} // metadata-only event, ignore
                    None => break, // notify watcher dropped
                }
            }
        }
    }
}

fn event_is_write(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    )
}

/// Async cookie read for the FS-event-triggered re-checks. Wraps
/// the sync `cookies_for_url` (which dispatches through the
/// webview's main thread) in `spawn_blocking` so the async runtime
/// never blocks waiting on it; bails after a 5s timeout if the
/// main thread is too busy. Returns:
/// - `Some(Some(t))` — read succeeded, cookie present
/// - `Some(None)`    — read succeeded, cookie absent (signed-out)
/// - `None`          — read failed / timed out (skip; retry on
///   the next filesystem event)
async fn try_read_cookie<R: Runtime>(
    window: WebviewWindow<R>,
    auth_url: Url,
    cookie_name: String,
) -> Option<Option<String>> {
    let fut = spawn_blocking(move || window.cookies_for_url(auth_url));
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(Ok(cookies))) => Some(
            cookies
                .into_iter()
                .find(|c| c.name() == cookie_name)
                .map(|c| c.value().to_string()),
        ),
        _ => None,
    }
}

fn maybe_emit(last: &mut Option<String>, token: Option<String>) {
    if token == *last {
        return;
    }
    *last = token.clone();
    emit_signed_in(&token);
}

/// Decode the middle (payload) segment of a JWT as base64url JSON,
/// then map known claim names into [`SignedInInfo`]. Returns `None`
/// if the token isn't a JWT we can parse.
/// Decode the middle (payload) segment of a JWT as base64url JSON
/// and map known claim names into [`SignedInInfo`]. The xAI `sso`
/// JWT carries only `session_id` today; the rest of the fields are
/// extracted opportunistically in case the token gains them later.
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
