mod args;
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

/// Tauri-managed state — process-global notify signal that the
/// content webview's `on_page_load` callback fires to kick the
/// [`cookies_watcher`] into re-checking cookies right after every
/// navigation. Fires before WebView2's lazy cookie-store disk flush,
/// so sign-in / sign-out / team-creation detection lands in sub-
/// second time on any page nav.
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
}
