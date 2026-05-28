mod args;
pub mod cef;
mod cookies_watcher;
mod credentials;
mod post_create_dialog;
mod state;
mod stdio;
mod webview;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;

use clap::Parser;
use clap::error::ErrorKind;
use psychological_operations_browser_sdk::output::Output;
use tokio::sync::Notify;

use crate::stdio::{CookiesWatcherSlot, PendingAck, ReadyTx};

/// Tauri-managed state — process-global notify signal that
/// overlay-reported navigations fire to kick the [`cookies_watcher`]
/// into re-checking cookies right after every URL change. SPA navs
/// often coincide with cookie changes (a session-cookie-setting
/// action immediately followed by a `router.push`), so the kick
/// gets the watcher off its filesystem-debounce sooner than the
/// next `notify` event would.
///
/// Today the kick fires from [`crate::stdio::report_url`] (overlay
/// → Rust on every SPA nav). Phase 4 of the CEF integration will
/// add a `CefDisplayHandler::OnAddressChange` hook that fires it
/// too, covering full-document loads.
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

/// `--help`, `--version`, and the special
/// `DisplayHelpOnMissingArgumentOrSubcommand` case are clap's three
/// "informational" error kinds — they're not real errors, they're
/// success-with-text. Mirror the convention used in
/// `psychological-operations-cli/src/run.rs::is_informational`.
fn is_informational(e: &clap::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args = match args::Args::try_parse() {
        Ok(a) => a,
        Err(e) if is_informational(&e) => {
            let _ = Output::Help { text: e.to_string() }.emit();
            std::process::exit(0);
        }
        Err(e) => {
            let _ = Output::Error { error: e.to_string() }.emit();
            std::process::exit(e.exit_code());
        }
    };

    // NOTE: CEF (Chromium Embedded Framework) initialization is
    // deferred to the per-mode webview creation path
    // (`crate::webview::create_x_app`) so the `cache_root` carries
    // the correct per-mode subdirectory — for X-App `.../x-app/cef/`,
    // for a future psyop `.../psyop/<name>/cef/`. CEF's
    // `multi_threaded_message_loop` spawns a separate UI thread, so
    // calling `initialize` from inside Tauri's `setup` (rather than
    // pre-builder) is fine: the main thread stays free for Tauri's
    // event loop. We still teardown via `cef::shutdown` after
    // `.run()` returns iff init actually ran (defensive — startup
    // can fail before the webview is built).

    // Build the frontend-ready signal BEFORE the Tauri builder so we
    // can hand the receiver to the stdin reader (started inside
    // `setup`) while the sender lives in Tauri-managed state for
    // the `frontend_ready` command to consume.
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let ready_rx = Mutex::new(Some(ready_rx));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(args)
        .manage(ReadyTx(Mutex::new(Some(ready_tx))))
        .manage(PendingAck(Mutex::new(None)))
        .manage(CookiesWatcherSlot(Mutex::new(None)))
        .manage(WatcherKick::new())
        .invoke_handler(tauri::generate_handler![
            stdio::frontend_ready,
            stdio::stdio_respond,
            stdio::current_mode,
            stdio::current_signed_in,
            stdio::current_panel,
            stdio::current_user_id,
            stdio::process_post_create_html,
            stdio::report_url,
            stdio::set_production_app_count,
            stdio::store_x_app_credential,
        ])
        .setup(move |app| {
            // Eagerly create the X-App webview so the overlay is
            // available to receive `psyops:request` events.
            webview::create_x_app(app.handle())?;

            // Start the stdin reader. It blocks on `ready_rx.recv()`
            // before reading, so anything the host writes during
            // startup stays in the OS pipe until the overlay's
            // `frontend_ready` call.
            let rx = ready_rx
                .lock()
                .expect("ready_rx lock poisoned")
                .take()
                .expect("ready_rx already consumed");
            stdio::start(app.handle().clone(), rx);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // CEF teardown after Tauri's event loop returns. Only fire if
    // CEF was actually initialized (the per-mode webview path may
    // never have run e.g. if startup failed early). Browsers should
    // already be closed (the window-close handler in `webview` asks
    // CEF to close its browser before the parent surface goes away).
    if cef::is_initialized() {
        cef::shutdown();
    }
}
