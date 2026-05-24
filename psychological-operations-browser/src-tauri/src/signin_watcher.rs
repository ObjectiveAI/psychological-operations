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
//! Every state change publishes four side-effects:
//!   1. JSONL line on stdout via [`Output::SignedIn::emit`]
//!   2. process-global slot ([`current`]) — for `current_signed_in`
//!      Tauri command callers
//!   3. `psyops:signed_in` Tauri event to the panel webview — the
//!      instruction panel listens for this
//!   4. reflow of the X-App window's child webviews (panel resizes
//!      to 0 when signed-in, [`PANEL_HEIGHT`] when not) via
//!      [`crate::webview::reflow`]
//!
//! Additionally: on a `false → true` flip in X-App mode, navigate
//! the content webview back to `https://console.x.ai/` so we land
//! on the canonical post-sign-in page.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use psychological_operations_browser_sdk::mode::{self, Mode};
use psychological_operations_browser_sdk::output::{Output, SignedInInfo};
use serde::{Deserialize, Serialize};
use tauri::async_runtime::{JoinHandle, spawn, spawn_blocking};
use tauri::{AppHandle, Emitter, Manager, Runtime, Url};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::webview;

/// Tauri event name fired on every state flip.
const EVENT_SIGNED_IN: &str = "psyops:signed_in";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedInPayload {
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<SignedInInfo>,
}

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

/// Read the most recent sign-in state the watcher has observed.
/// `None` before the first emission (e.g. process started but no
/// mode-setting request has run yet).
pub fn current() -> Option<SignedInPayload> {
    current_slot()
        .lock()
        .expect("signed_in slot poisoned")
        .clone()
}

fn current_slot() -> &'static Mutex<Option<SignedInPayload>> {
    static SLOT: OnceLock<Mutex<Option<SignedInPayload>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Start the sign-in watcher for the given mode. Returns `None`
/// for modes we haven't wired up yet (currently only [`Mode::XApp`]
/// is supported). Performs an initial synchronous cookie read +
/// state emission before spawning the async filesystem-watch task.
pub fn start<R: Runtime>(
    handle: AppHandle<R>,
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

    // Initial sync read — main thread is free now (we're on the
    // stdio reader thread, no JS dispatch in flight).
    let initial_token = read_cookie_sync(&handle, auth_url.clone(), &cookie_name);
    emit_signed_in(&handle, &initial_token);

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
    let handle_for_task = handle.clone();
    let task = spawn(async move {
        run_watcher(
            handle_for_task,
            auth_url,
            cookie_name,
            initial_token,
            fs_rx,
            stop_for_task,
        )
        .await;
    });

    Some(Handle {
        _watcher: watcher,
        stop,
        _task: task,
    })
}

/// Look up the content webview's cookies for `auth_url` synchronously.
/// Returns the auth-cookie value if present, `None` otherwise.
fn read_cookie_sync<R: Runtime>(
    handle: &AppHandle<R>,
    auth_url: Url,
    cookie_name: &str,
) -> Option<String> {
    let webview = handle.get_webview(webview::CONTENT_LABEL)?;
    match webview.cookies_for_url(auth_url) {
        Ok(cookies) => cookies
            .into_iter()
            .find(|c| c.name() == cookie_name)
            .map(|c| c.value().to_string()),
        Err(e) => {
            let _ = Output::Log {
                message: format!("signin_watcher: cookies_for_url err: {e}"),
            }
            .emit();
            None
        }
    }
}

/// Publishes the current sign-in state through every channel that
/// cares: stdout, the process-global slot, the panel webview's
/// Tauri-event listener, the window reflow, and the post-sign-in
/// content-webview redirect.
fn emit_signed_in<R: Runtime>(handle: &AppHandle<R>, token: &Option<String>) {
    let prev_signed_in = current().map(|s| s.signed_in);

    let info = token.as_deref().and_then(jwt_to_info);
    let payload = SignedInPayload {
        signed_in: token.is_some(),
        info,
    };

    let _ = Output::SignedIn {
        signed_in: payload.signed_in,
        info: payload.info.clone(),
    }
    .emit();

    *current_slot()
        .lock()
        .expect("signed_in slot poisoned") = Some(payload.clone());

    // The panel webview's React listener picks this up.
    let _ = handle.emit_to(webview::PANEL_LABEL, EVENT_SIGNED_IN, &payload);

    // Resize the panel webview based on the new state.
    webview::reflow(handle);

    // Post-sign-in redirect: when state flips false → true in X-App
    // mode, bounce the content webview back to console.x.ai so we
    // land on the canonical signed-in page even if the OAuth flow
    // left us in an in-between origin.
    if prev_signed_in == Some(false)
        && payload.signed_in
        && matches!(mode::get(), Some(Mode::XApp))
    {
        if let Some(content) = handle.get_webview(webview::CONTENT_LABEL) {
            if let Ok(target) = Url::parse("https://console.x.ai/") {
                let _ = content.navigate(target);
            }
        }
    }
}

async fn run_watcher<R: Runtime>(
    handle: AppHandle<R>,
    auth_url: Url,
    cookie_name: String,
    initial_token: Option<String>,
    mut fs_rx: UnboundedReceiver<Event>,
    stop: Arc<Notify>,
) {
    let mut last: Option<String> = initial_token;

    let kick = handle.state::<WatcherKick>().0.clone();

    loop {
        tokio::select! {
            _ = stop.notified() => break,
            // Filesystem-driven trigger — eventually catches the
            // sign-out cookie clear when WebView2 flushes its
            // in-memory store to disk (lazy; can be 5-30s).
            ev = fs_rx.recv() => {
                match ev {
                    Some(e) if event_is_write(&e) => {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        while fs_rx.try_recv().is_ok() {}
                        if let Some(state) = try_read_cookie(
                            handle.clone(),
                            auth_url.clone(),
                            cookie_name.clone(),
                        )
                        .await
                        {
                            maybe_emit(&handle, &mut last, state);
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            // Navigation-driven trigger — fired by the content
            // webview's `on_page_load(Finished)` callback (see
            // `crate::webview`). cookies_for_url queries WebView2's
            // in-memory store directly, so this catches sign-in /
            // sign-out flips immediately on the page navigation they
            // typically cause — well before WebView2 gets around to
            // flushing the disk cookie store.
            _ = kick.notified() => {
                if let Some(state) = try_read_cookie(
                    handle.clone(),
                    auth_url.clone(),
                    cookie_name.clone(),
                )
                .await
                {
                    maybe_emit(&handle, &mut last, state);
                }
            }
        }
    }
}

/// Tauri state — process-global notify signal that the content
/// webview's `on_page_load` callback fires to kick the watcher into
/// re-checking cookies right after every navigation. Fires before
/// WebView2's lazy cookie-store disk flush, so sign-in / sign-out
/// detection lands in sub-second time on any page nav.
pub struct WatcherKick(pub Arc<Notify>);

impl WatcherKick {
    pub fn new() -> Self {
        Self(Arc::new(Notify::new()))
    }
}

impl Default for WatcherKick {
    fn default() -> Self {
        Self::new()
    }
}


fn event_is_write(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    )
}

/// Async cookie read for FS-event-triggered re-checks.
///   Some(Some(t)) → read succeeded, cookie present
///   Some(None)    → read succeeded, cookie absent (signed-out)
///   None          → read failed / timed out (skip; retry next event)
async fn try_read_cookie<R: Runtime>(
    handle: AppHandle<R>,
    auth_url: Url,
    cookie_name: String,
) -> Option<Option<String>> {
    let fut = spawn_blocking(move || {
        handle
            .get_webview(webview::CONTENT_LABEL)?
            .cookies_for_url(auth_url)
            .ok()
    });
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(Some(cookies))) => Some(
            cookies
                .into_iter()
                .find(|c| c.name() == cookie_name)
                .map(|c| c.value().to_string()),
        ),
        _ => None,
    }
}

fn maybe_emit<R: Runtime>(
    handle: &AppHandle<R>,
    last: &mut Option<String>,
    token: Option<String>,
) {
    if token == *last {
        return;
    }
    *last = token.clone();
    emit_signed_in(handle, &token);
}

/// Decode JWT payload claims into [`SignedInInfo`]. The xAI `sso`
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
