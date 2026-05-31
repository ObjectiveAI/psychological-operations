mod args;
pub mod cef;
mod cef_scheme;
mod cef_v8;
mod cookies_watcher;
mod credentials;
mod psyop_authorize;
mod psyop_read;
mod state;
mod stdio;
mod webview;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;

use clap::Parser;
use clap::error::ErrorKind;
use psychological_operations_sdk::browser::mode;
use psychological_operations_sdk::browser::output::Output;
use tauri::Manager;
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

    // The CLI mode flag is required (clap's ArgGroup enforces it).
    // Lock the SDK mode static once for the lifetime of the
    // process — there is no runtime mode switch.
    let initial_mode = args.initial_mode();
    mode::set(initial_mode.clone());

    // Build the frontend-ready signal BEFORE the Tauri builder so
    // we can hand the receiver to the stdin reader (started inside
    // `setup`) while the sender lives in Tauri-managed state for
    // the first `frontend_ready` invoke to consume. Subsequent
    // mode switches replace the sender via `ReadyTx` mutate.
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let ready_rx = Mutex::new(Some(ready_rx));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(args)
        .manage(ReadyTx(Mutex::new(Some(ready_tx))))
        .manage(PendingAck(Mutex::new(None)))
        .manage(CookiesWatcherSlot(Mutex::new(None)))
        .manage(WatcherKick::new())
        // Only the panel webview uses Tauri IPC; the CEF content
        // overlay uses the `psyops://` scheme (see `cef_scheme`).
        // So `current_panel` is the sole registered command.
        .invoke_handler(tauri::generate_handler![stdio::current_panel])
        .setup(move |app| {
            let handle = app.handle();

            // 1. Build the Tauri window + panel webview + CEF
            //    browser scoped to the locked mode's RequestContext.
            //    CEF's shared root cache is initialized inside
            //    `webview::create_x_app`.
            webview::create_x_app(handle, &initial_mode)?;

            // 2. Start the cookies watcher for the locked mode.
            let watcher_slot: tauri::State<CookiesWatcherSlot> = handle.state();
            let data_dir = webview::mode_data_dir(handle, &initial_mode);
            *watcher_slot.0.lock().expect("watcher slot poisoned") =
                cookies_watcher::start(handle.clone(), &initial_mode, &data_dir);

            // 3. Start the stdin reader. It blocks on
            //    `ready_rx.recv()` before reading, so anything the
            //    host writes during startup stays in the OS pipe
            //    until the overlay's `frontend_ready` call.
            let rx = ready_rx
                .lock()
                .expect("ready_rx lock poisoned")
                .take()
                .expect("ready_rx already consumed");
            stdio::start(handle.clone(), rx);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // CEF teardown after Tauri's event loop returns. Browsers
    // should already be closed (the window-close handler asks CEF
    // to close before the parent surface goes away).
    if cef::is_initialized() {
        cef::shutdown();
    }
}
