mod args;
mod authorize;
pub mod cef;
mod cef_scheme;
mod cef_v8;
mod cookies_watcher;
mod credentials;
mod deliver;
mod psyop_read;
mod state;
mod stdio;
mod webview;

use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;

use clap::error::ErrorKind;
use clap::Parser;
use psychological_operations_sdk::browser::deliver::DeliverItem;
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
            let _ = Output::Help {
                text: e.to_string(),
            }
            .emit();
            std::process::exit(0);
        }
        Err(e) => {
            let _ = Output::Error {
                error: e.to_string(),
            }
            .emit();
            std::process::exit(e.exit_code());
        }
    };

    // Reply/quote delivery (`--deliver <json>`) is a separate invocation,
    // NOT a persona `Mode` — the batch spans agents, so it skips the Mode
    // system and runs the delivery driver (per-agent CEF sessions) instead
    // of the persona UI. Parse the inline item array up front.
    let deliver_items: Option<Vec<DeliverItem>> = match args.deliver.as_ref() {
        Some(json) => match serde_json::from_str::<Vec<DeliverItem>>(json) {
            Ok(items) => Some(items),
            Err(e) => {
                let _ = Output::Error {
                    error: format!("--deliver: invalid JSON: {e}"),
                }
                .emit();
                std::process::exit(2);
            }
        },
        None => None,
    };

    // The CLI mode flag is required (clap's ArgGroup enforces it). Lock the
    // SDK mode static once — but only when NOT delivering (delivery has no
    // persona mode; `mode::get()` stays `None`).
    let initial_mode = if deliver_items.is_none() {
        let m = args.initial_mode();
        mode::set(m.clone());
        Some(m)
    } else {
        None
    };

    // Connect the persistence layer up front (credential-HTML + token
    // storage). Uses tauri's global async runtime since the builder
    // hasn't started yet. Fatal on failure.
    let db = match tauri::async_runtime::block_on(psychological_operations_db::Db::connect(
        &args.postgres_url,
    )) {
        Ok(db) => db,
        Err(e) => {
            let _ = Output::Error {
                error: format!("db connect: {e}"),
            }
            .emit();
            std::process::exit(1);
        }
    };

    // Build the frontend-ready signal BEFORE the Tauri builder so
    // we can hand the receiver to the stdin reader (started inside
    // `setup`) while the sender lives in Tauri-managed state for
    // the first `frontend_ready` invoke to consume. Subsequent
    // mode switches replace the sender via `ReadyTx` mutate.
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let ready_rx = Mutex::new(Some(ready_rx));
    // Whether this process is a delivery run — drives the `RunEvent`
    // exit guard below (a per-agent browser close must not tear the app
    // down mid-batch).
    let is_deliver = deliver_items.is_some();
    // Moved into the setup closure; `take()`-n there to pick the path.
    let deliver_items = Mutex::new(deliver_items);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(args)
        .manage(db)
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

            // The stdin reader (started in both paths) blocks on
            // `ready_rx.recv()` before reading, so host writes during
            // startup stay buffered in the OS pipe until `frontend_ready`.
            let rx = ready_rx
                .lock()
                .expect("ready_rx lock poisoned")
                .take()
                .expect("ready_rx already consumed");

            let items = deliver_items
                .lock()
                .expect("deliver_items lock poisoned")
                .take();

            if let Some(items) = items {
                // Delivery path: window shell (no per-mode browser — the
                // driver opens one per agent), the stdin reader (so the dev
                // bridge's Html/Eval introspection works), and the driver.
                webview::create_deliver_window(handle)?;
                stdio::start(handle.clone(), rx);
                deliver::start(handle.clone(), items);
            } else {
                // Persona path: the window + the single mode-scoped CEF
                // browser + the cookies watcher.
                let mode = initial_mode
                    .as_ref()
                    .expect("initial_mode is set when not delivering");
                webview::create_x_app(handle, mode)?;
                let watcher_slot: tauri::State<CookiesWatcherSlot> = handle.state();
                *watcher_slot.0.lock().expect("watcher slot poisoned") =
                    cookies_watcher::start(handle.clone(), mode);
                stdio::start(handle.clone(), rx);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(move |_handle, event| {
        // In delivery mode the driver opens AND closes one CEF browser per
        // agent inside a single window; closing a browser between agents
        // otherwise lets Tauri's default "last surface gone → exit" tear
        // the whole process down after the first agent. Hold off the exit
        // until the driver flags itself finished — its own `handle.exit(0)`
        // (after `deliver::mark_finished`) is then the sole terminator.
        if is_deliver && !deliver::is_finished() {
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                api.prevent_exit();
            }
        }
    });

    // CEF teardown after Tauri's event loop returns. Browsers
    // should already be closed (the window-close handler asks CEF
    // to close before the parent surface goes away).
    if cef::is_initialized() {
        cef::shutdown();
    }
}
