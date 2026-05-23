//! `psychological-operations-browser` — Tauri webview shell.
//!
//! `lib.rs` exposes the canonical `run()` entry point per the official
//! `cargo create-tauri-app` template. It parses CLI args, sets up
//! per-session state isolation (Windows: WEBVIEW2_USER_DATA_FOLDER),
//! and runs the Tauri builder.

mod args;
mod browser;
mod pointer;
mod stdin_io;

use clap::Parser;

pub use crate::args::Args;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args = Args::parse();
    if let Err(e) = args.validate() {
        eprintln!("ERROR: {e:#}");
        std::process::exit(1);
    }

    // Per-session state isolation. On Windows, WebView2 honors the
    // WEBVIEW2_USER_DATA_FOLDER env var for its data folder
    // (cookies, IndexedDB, LocalStorage, cache, etc.). We set it
    // before Tauri spins up the webview.
    //
    // macOS / Linux: webview backends use the app's data dir by
    // default; per-session isolation on those is a TODO.
    let target_dir = args.target_dir();
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        eprintln!("ERROR: creating target dir {}: {e}", target_dir.display());
        std::process::exit(1);
    }
    // SAFETY: env vars are process-global; we set this from main
    // before any webview-spawning code runs, so there's no race.
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &target_dir);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![stdin_io::psyops_html_response])
        .setup(move |app| {
            let handle = app.handle().clone();
            crate::browser::build_window(&handle, &args)?;
            crate::stdin_io::start(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
