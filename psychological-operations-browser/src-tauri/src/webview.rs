//! Tauri webview construction.
//!
//! The X-App webview loads `https://x.com` directly and has our
//! React overlay bundle injected at document-creation time via
//! [`WebviewWindowBuilder::initialization_script`] (which maps to
//! WebView2's `AddScriptToExecuteOnDocumentCreated` on Windows).
//! Native full-page navigations route through the `on_navigation`
//! callback, which emits [`Output::Url`].
//!
//! Other modes (psyop sessions) will land in this file alongside
//! `create_x_app` once the wire shape settles.

use psychological_operations_browser_sdk::output::Output;
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::args::Args;

/// Label used to look up / build the X-App webview window.
pub const X_APP_LABEL: &str = "x-app";

/// Self-contained IIFE bundle of the React overlay, baked in at
/// compile time. Produced by `pnpm build` (Vite) at
/// `psychological-operations-browser/dist/overlay.js` — so a
/// frontend build must run before this crate compiles.
const OVERLAY_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../dist/overlay.js"
));

/// Create the X-App webview if it doesn't already exist. Loads
/// `https://x.com` directly, injects the React overlay, persists
/// session state to `<config-base-dir>/plugins/psychological-operations/browser/x-app/`.
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

    let url = Url::parse("https://x.com").expect("hardcoded URL parses");

    WebviewWindowBuilder::new(handle, X_APP_LABEL, WebviewUrl::External(url))
        .title("psychological-operations-browser — X-App")
        .data_directory(data_dir)
        .initialization_script(OVERLAY_JS)
        .inner_size(1200.0, 800.0)
        .on_navigation(|url| {
            let _ = Output::Url {
                url: url.to_string(),
            }
            .emit();
            true
        })
        .build()
}
