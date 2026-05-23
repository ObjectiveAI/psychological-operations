//! Tauri webview construction.
//!
//! The X-App webview loads `https://console.x.ai/` directly (the
//! X developer console — same landing page the old chromium fork
//! used for `--x-app` mode) and has our React overlay bundle
//! injected at document-creation time via
//! [`WebviewWindowBuilder::initialization_script`] (which maps to
//! WebView2's `AddScriptToExecuteOnDocumentCreated` on Windows).
//!
//! URL emission is the frontend's responsibility — there is no
//! Rust-side `on_navigation` callback. The injected overlay calls
//! the `report_url` Tauri command in [`crate::stdio`] for both the
//! initial URL and every subsequent change.

use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::args::Args;

/// Label used to look up / build the X-App webview window.
pub const X_APP_LABEL: &str = "x-app";

/// Self-contained IIFE bundle of the React overlay, baked in at
/// compile time. Produced by `yarn build` (Vite) at
/// `psychological-operations-browser/dist/overlay.js` — so a
/// frontend build must run before this crate compiles.
const OVERLAY_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../dist/overlay.js"
));

/// Create the X-App webview if it doesn't already exist. Loads
/// `https://console.x.ai/` directly, injects the React overlay,
/// persists session state to
/// `<config-base-dir>/plugins/psychological-operations/browser/x-app/`.
/// Idempotent — returns the existing webview if one is alive.
pub fn create_x_app<R: Runtime>(handle: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    if let Some(w) = handle.get_webview_window(X_APP_LABEL) {
        return Ok(w);
    }

    let data_dir = {
        let args = handle.state::<Args>();
        args.config_base_dir
            .join("plugins")
            .join("psychological-operations")
            .join("browser")
            .join("x-app")
    };
    std::fs::create_dir_all(&data_dir)?;

    let url = Url::parse("https://console.x.ai/").expect("hardcoded URL parses");

    WebviewWindowBuilder::new(handle, X_APP_LABEL, WebviewUrl::External(url))
        .title("psychological-operations-browser — X-App")
        .data_directory(data_dir)
        .initialization_script(OVERLAY_JS)
        .inner_size(1200.0, 800.0)
        .build()
}
