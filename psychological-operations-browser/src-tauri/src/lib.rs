pub mod args;
pub mod browser;
pub mod pointer;
pub mod stdin_io;

use anyhow::Result;

use crate::args::Args;

pub fn run(args: Args) -> Result<()> {
    args.validate()?;

    // On Windows, WebView2 honors WEBVIEW2_USER_DATA_FOLDER for the
    // user-data folder of the embedded webview. Setting it before
    // Tauri initializes the webview pins all of the session's storage
    // (cookies, IndexedDB, LocalStorage, cache, ServiceWorker
    // registrations) under the per-session target dir.
    //
    // macOS / Linux: webview backends use the app's data dir by
    // default; per-session isolation on those platforms is a TODO.
    let target_dir = args.target_dir();
    std::fs::create_dir_all(&target_dir).ok();
    // SAFETY: set_var is unsafe in edition 2024 because env vars are
    // process-global; we set it from the main thread before any
    // webview-spawning code runs, so there's no race.
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &target_dir);
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![stdin_io::psyops_html_response])
        .setup(move |app| {
            let handle = app.handle().clone();
            let _window = browser::build_window(&handle, &args)?;
            stdin_io::start(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(Into::into)
}
