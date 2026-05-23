mod args;
mod stdio;
mod webview;

use std::sync::Mutex;
use std::sync::mpsc;

use clap::Parser;
use clap::error::ErrorKind;
use psychological_operations_browser_sdk::output::Output;

use crate::stdio::{CurrentMode, PendingAck, ReadyTx};

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
        .manage(CurrentMode(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            stdio::frontend_ready,
            stdio::stdio_respond,
            stdio::current_mode,
            stdio::report_url,
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
