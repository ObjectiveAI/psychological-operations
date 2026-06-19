//! Cookie watcher — turns raw cookie observations into [`crate::state`]
//! setter calls.
//!
//! Per the user's spec: "rust will listen to changes to this specific
//! cookie. cookies may change that are not the one we care about — we
//! don't need to check anything unless that particular cookie
//! changes." The mechanics:
//!
//!   - **Navigation kick** (via [`crate::WatcherKick`], an
//!     `Arc<Notify>`). Today fired by [`crate::stdio::report_url`]
//!     on every overlay-reported SPA navigation. Phase 4 will
//!     additionally wire a `CefDisplayHandler::OnAddressChange` kick
//!     for full-document loads.
//!
//! On each kick we snapshot the cookies we care about (via
//! [`crate::cef::snapshot_cookies`] which round-trips through CEF's
//! `CookieManager` on the UI thread) and push them into the
//! [`crate::state`] fact slots. The state module owns the "did
//! anything change → emit" decision and all four side-effects
//! (stdout, Tauri event, reflow, redirect). This module owns nothing
//! but the I/O: reading cookies, dispatching to state.
//!
//! Note: the pre-CEF implementation also used a filesystem watcher
//! (`notify` crate) on the WebView2 cookie SQLite directory because
//! WebView2's in-memory cookie store lazily flushed to disk (5–30 s).
//! CEF's `CookieManager` reads its in-memory store immediately, so
//! the FS watch is no longer needed.
//!
//! Adding a new cookie to observe: add a field to [`CookieSnapshot`],
//! extract it in [`snapshot_sync`], call the matching `state::set_*`
//! in [`apply_snapshot`]. The state module + panel derivation pick
//! it up.

use std::sync::Arc;
use std::time::Duration;

use psychological_operations_db::parse_twid;
use psychological_operations_sdk::browser::mode::Mode;
use tauri::async_runtime::{JoinHandle, spawn, spawn_blocking};
use tauri::{AppHandle, Manager, Url, Wry};
use tokio::sync::Notify;

use crate::WatcherKick;
use crate::cef;
use crate::state;

/// All cookies the panel-state derivation cares about, in one place.
/// One `snapshot_cookies(url)` call returns every cookie CEF has for
/// the URL's domain, so we always grab them together.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CookieSnapshot {
    /// x.com's HttpOnly session cookie on `.x.com`. Presence means
    /// the user is signed in to x.com / console.x.com.
    auth_token: Option<String>,
    /// X user-id parsed from the `twid` cookie. Used by the
    /// overlay's credential-storage flow as the per-user folder
    /// key (different sign-ins → different twid → different
    /// folders). Stays stable for the lifetime of a session.
    user_id: Option<String>,
}

pub struct Handle {
    stop: Arc<Notify>,
    _task: JoinHandle<()>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

/// Start the cookies watcher for the given mode. Both X-App and
/// agent modes watch the same x.com cookies (`auth_token`, `twid`) —
/// per-mode isolation comes from each browser's own
/// `RequestContext`, not from watching different URLs. Performs
/// an initial synchronous read + dispatch before spawning the
/// kick loop.
pub fn start(
    handle: AppHandle<Wry>,
    _mode: &Mode,
) -> Option<Handle> {
    let auth_url: Url = Url::parse("https://x.com/").ok()?;

    // Initial sync read — main thread is free here (we're on the
    // stdio reader thread, no JS dispatch in flight). The
    // async-only `apply_snapshot` runs inside the spawned task
    // below so `start` itself stays sync (called from Tauri's
    // sync `setup` closure).
    let initial = snapshot_sync(&auth_url);

    let stop = Arc::new(Notify::new());
    let stop_for_task = stop.clone();
    let handle_for_task = handle.clone();
    let task = spawn(async move {
        apply_snapshot(&handle_for_task, &initial).await;
        run_watcher(handle_for_task, auth_url, initial, stop_for_task).await;
    });

    Some(Handle {
        stop,
        _task: task,
    })
}

/// Read every cookie of interest in one call. Synchronous; blocks
/// up to 5 s on CEF's UI-thread round-trip. Returns empty on
/// timeout / CEF not initialized — the next kick retries.
fn snapshot_sync(auth_url: &Url) -> CookieSnapshot {
    let pairs = cef::snapshot_cookies(auth_url.as_str());
    let mut snap = CookieSnapshot::default();
    for (name, value) in pairs {
        match name.as_str() {
            "auth_token" => snap.auth_token = Some(value),
            "twid" => snap.user_id = parse_twid(&value),
            _ => {}
        }
    }
    snap
}

/// Push every fact from a fresh snapshot into the [`crate::state`]
/// store, THEN kick the authorize OAuth flow. These run SEQUENTIALLY,
/// not concurrently, and the order is load-bearing: `maybe_start_flow`
/// reads `state::current_user_id` (i.e. `Facts::user_id`), which
/// `apply_cookie_facts` only writes AFTER its disk/db await. Under the
/// old `tokio::join!`, `maybe_start_flow` was polled while
/// `apply_cookie_facts` was still suspended on that await, so it
/// observed `user_id == None` and bailed — on the one snapshot where
/// sign-in flips empty→twid (the dedup in `maybe_apply` then never
/// retried it), so the OAuth consent never fired. Applying facts first
/// guarantees `user_id` is set before the flow checks it.
/// `maybe_start_flow` no-ops outside the Agent authorize mode.
async fn apply_snapshot(handle: &AppHandle<Wry>, snap: &CookieSnapshot) {
    state::apply_cookie_facts(handle, snap.auth_token.clone(), snap.user_id.clone()).await;
    crate::authorize::maybe_start_flow(handle).await;
}

async fn run_watcher(
    handle: AppHandle<Wry>,
    auth_url: Url,
    initial: CookieSnapshot,
    stop: Arc<Notify>,
) {
    let mut last = initial;
    let kick = handle.state::<WatcherKick>().0.clone();

    // Drain any pre-existing kick permits. The initial `snapshot_sync`
    // above already captured the current state, so a stored permit
    // (from a `report_url` that fired before this watcher task
    // started) would only cause a redundant read. Worse: that read,
    // racing with the rest of the x_app dispatch (emit + ack roundtrip
    // through the main UI thread + multi-webview set_size dispatches),
    // deadlocks the main thread on this build of Tauri. See commit
    // 17d97b3 for the original diagnosis.
    loop {
        tokio::select! {
            biased;
            _ = kick.notified() => continue,
            _ = std::future::ready(()) => break,
        }
    }

    loop {
        tokio::select! {
            _ = stop.notified() => break,
            // Navigation-driven trigger — fired by the overlay's
            // `report_url` invoke on every SPA nav (and Phase 4
            // will add a CEF DisplayHandler::OnAddressChange kick
            // for full-document loads). Catches sign-in / sign-out
            // flips immediately on the page nav they cause.
            _ = kick.notified() => {
                if let Some(snap) =
                    try_snapshot(auth_url.clone()).await
                {
                    maybe_apply(&handle, &mut last, snap).await;
                }
            }
        }
    }
}

/// Async snapshot for kick-triggered re-reads. Wraps the sync call
/// (which blocks up to 5 s on CEF's UI thread) in `spawn_blocking`
/// so the watcher task itself never blocks. Returns `None` on
/// timeout/join-error — the next kick retries.
async fn try_snapshot(auth_url: Url) -> Option<CookieSnapshot> {
    let fut = spawn_blocking(move || snapshot_sync(&auth_url));
    match tokio::time::timeout(Duration::from_secs(6), fut).await {
        Ok(Ok(snap)) => Some(snap),
        _ => None,
    }
}

async fn maybe_apply(
    handle: &AppHandle<Wry>,
    last: &mut CookieSnapshot,
    snap: CookieSnapshot,
) {
    if snap == *last {
        return;
    }
    *last = snap.clone();
    apply_snapshot(handle, &snap).await;
}
