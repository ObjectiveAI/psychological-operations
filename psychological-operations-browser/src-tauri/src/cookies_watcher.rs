//! Cookie watcher — turns raw cookie observations into [`crate::state`]
//! setter calls.
//!
//! Per the user's spec: "rust will listen to changes to this specific
//! cookie. cookies may change that are not the one we care about — we
//! don't need to check anything unless that particular cookie
//! changes." The mechanics:
//!
//!   - **Filesystem watch** (via the `notify` crate) on the WebView2
//!     cookie SQLite directory at
//!     `<data-dir>/EBWebView/Default/Network/`. Each write event
//!     triggers a re-read.
//!   - **Navigation kick** (via [`crate::WatcherKick`], an
//!     `Arc<Notify>` fired by the content webview's `on_page_load`
//!     callback). `cookies_for_url` queries WebView2's in-memory
//!     store directly, so we catch sign-in / sign-out / team-creation
//!     flips immediately on the page nav they typically cause — well
//!     before WebView2's lazy disk flush (5–30s).
//!
//! On each tick, we snapshot the cookies we care about and push them
//! into the [`crate::state`] fact slots. The state module owns the
//! "did anything change → emit" decision and all four side-effects
//! (stdout, Tauri event, reflow, redirect). This module owns nothing
//! but the I/O: reading cookies, dispatching to state.
//!
//! Adding a new cookie to observe: add a field to [`CookieSnapshot`],
//! extract it in [`snapshot_sync`], call the matching `state::set_*`
//! in [`apply_snapshot`]. The state module + panel derivation pick
//! it up.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use psychological_operations_browser_sdk::mode::Mode;
use psychological_operations_browser_sdk::output::Output;
use tauri::async_runtime::{JoinHandle, spawn, spawn_blocking};
use tauri::{AppHandle, Manager, Runtime, Url};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::WatcherKick;
use crate::state;
use crate::webview;

/// All cookies the panel-state derivation cares about, in one place.
/// One `cookies_for_url` call returns every cookie for the URL's
/// domain, so we always grab them together.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CookieSnapshot {
    sso: Option<String>,
    last_team_id: Option<String>,
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

/// Start the cookies watcher for the given mode. Returns `None` for
/// modes we haven't wired up yet (currently only [`Mode::XApp`]).
/// Performs an initial synchronous read + dispatch before spawning
/// the async watcher task.
pub fn start<R: Runtime>(
    handle: AppHandle<R>,
    mode: &Mode,
    data_dir: &Path,
) -> Option<Handle> {
    let auth_url: Url = match mode {
        Mode::XApp => Url::parse("https://console.x.ai/").ok()?,
        Mode::Psyop { .. } => {
            let _ = Output::Log {
                message: "cookies_watcher: Psyop mode not yet wired".into(),
            }
            .emit();
            return None;
        }
    };

    let cookie_store_dir = data_dir.join("EBWebView").join("Default").join("Network");
    let _ = std::fs::create_dir_all(&cookie_store_dir);

    // Initial sync read — main thread is free here (we're on the stdio
    // reader thread, no JS dispatch in flight).
    let initial = snapshot_sync(&handle, &auth_url);
    apply_snapshot(&handle, &initial);

    let (fs_tx, fs_rx) = unbounded_channel::<Event>();
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            let _ = fs_tx.send(ev);
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            let _ = Output::Log {
                message: format!("cookies_watcher: failed to create watcher: {e}"),
            }
            .emit();
            return None;
        }
    };
    if let Err(e) = watcher.watch(&cookie_store_dir, RecursiveMode::NonRecursive) {
        let _ = Output::Log {
            message: format!(
                "cookies_watcher: failed to watch {}: {e}",
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
        run_watcher(handle_for_task, auth_url, initial, fs_rx, stop_for_task).await;
    });

    Some(Handle {
        _watcher: watcher,
        stop,
        _task: task,
    })
}

/// Read every cookie of interest in one `cookies_for_url` call.
fn snapshot_sync<R: Runtime>(handle: &AppHandle<R>, auth_url: &Url) -> CookieSnapshot {
    let Some(webview) = handle.get_webview(webview::CONTENT_LABEL) else {
        return CookieSnapshot::default();
    };
    let cookies = match webview.cookies_for_url(auth_url.clone()) {
        Ok(c) => c,
        Err(e) => {
            let _ = Output::Log {
                message: format!("cookies_watcher: cookies_for_url err: {e}"),
            }
            .emit();
            return CookieSnapshot::default();
        }
    };
    let mut snap = CookieSnapshot::default();
    for c in &cookies {
        match c.name() {
            "sso" => snap.sso = Some(c.value().to_string()),
            "last-team-id" => snap.last_team_id = Some(c.value().to_string()),
            _ => {}
        }
    }
    snap
}

/// Push every fact from a fresh snapshot into the [`crate::state`]
/// store. Atomic — all cookie facts land in the store together so
/// no intermediate `PanelState` ever leaks out between, e.g., "sso
/// observed but `last-team-id` not yet read."
fn apply_snapshot<R: Runtime>(handle: &AppHandle<R>, snap: &CookieSnapshot) {
    state::apply_cookie_facts(handle, snap.sso.clone(), snap.last_team_id.clone());
}

async fn run_watcher<R: Runtime>(
    handle: AppHandle<R>,
    auth_url: Url,
    initial: CookieSnapshot,
    mut fs_rx: UnboundedReceiver<Event>,
    stop: Arc<Notify>,
) {
    let mut last = initial;
    let kick = handle.state::<WatcherKick>().0.clone();

    // Drain any pre-existing kick permits. The initial `snapshot_sync`
    // above already captured the current state, so a stored permit
    // (from an `on_page_load` that fired before this watcher task
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
            // Filesystem-driven trigger — eventually catches cookie
            // changes when WebView2 flushes its in-memory store to
            // disk (lazy; can be 5–30s).
            ev = fs_rx.recv() => {
                match ev {
                    Some(e) if event_is_write(&e) => {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        while fs_rx.try_recv().is_ok() {}
                        if let Some(snap) =
                            try_snapshot(handle.clone(), auth_url.clone()).await
                        {
                            maybe_apply(&handle, &mut last, snap);
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            // Navigation-driven trigger — fired by the content
            // webview's `on_page_load(Finished)` callback. Catches
            // sign-in / sign-out / team-creation flips immediately on
            // the page nav they cause.
            _ = kick.notified() => {
                if let Some(snap) =
                    try_snapshot(handle.clone(), auth_url.clone()).await
                {
                    maybe_apply(&handle, &mut last, snap);
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

/// Async snapshot for fs-event-triggered re-reads (off the stdio
/// thread). Wraps the sync call in `spawn_blocking` with a 5-second
/// timeout. Returns `None` on timeout — the next tick will retry.
async fn try_snapshot<R: Runtime>(
    handle: AppHandle<R>,
    auth_url: Url,
) -> Option<CookieSnapshot> {
    let fut = spawn_blocking(move || snapshot_sync(&handle, &auth_url));
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(snap)) => Some(snap),
        _ => None,
    }
}

fn maybe_apply<R: Runtime>(
    handle: &AppHandle<R>,
    last: &mut CookieSnapshot,
    snap: CookieSnapshot,
) {
    if snap == *last {
        return;
    }
    *last = snap.clone();
    apply_snapshot(handle, &snap);
}
