//! Tauri window + child-webview construction.
//!
//! One `Window` (label `x-app`) with two child `Webview`s stacked:
//!
//!   - `PANEL_LABEL` ("panel") at the top, sized to [`PANEL_HEIGHT`]
//!     when the derived [`crate::state::PanelState`] is `Show`, 0
//!     when `Hidden` (or before any derivation has run). Loads our
//!     local Vite-built `panel.html` (tauri:// asset protocol) so
//!     Tauri IPC works unconditionally — no remote-URL ACL needed.
//!
//!   - `CONTENT_LABEL` ("content") below the panel, taking the
//!     remaining height. Loads `https://console.x.com/` directly with
//!     the React/non-React overlay bundle injected via
//!     `WebviewBuilder::initialization_script` (which maps to
//!     WebView2's `AddScriptToExecuteOnDocumentCreated` on Windows).
//!     The overlay handles stdio request dispatch, SPA URL
//!     reporting, console capture; the instruction panel is NOT in
//!     this bundle (it lives in the panel webview).
//!
//! Layout reflow happens (a) on window resize and (b) on every
//! derived-state flip via [`reflow`], called from
//! [`crate::state::recompute_and_publish`].

use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalSize, Runtime, Url, WebviewUrl,
    Window, WindowEvent,
};

use crate::WatcherKick;
use crate::args::Args;
use crate::state;

/// Label of the single Tauri Window the X-App lives in.
pub const X_APP_WINDOW: &str = "x-app";

/// Label of the panel child webview (top of the window).
pub const PANEL_LABEL: &str = "panel";

/// Label of the content child webview (rest of the window).
pub const CONTENT_LABEL: &str = "content";

/// Logical pixel height of the panel webview when visible.
const PANEL_HEIGHT: u32 = 48;
const DEFAULT_WIDTH: u32 = 1200;
const DEFAULT_HEIGHT: u32 = 800;

/// Self-contained IIFE bundle of the overlay (no React, no
/// InstructionPanel — those moved to the panel webview). Produced
/// by `yarn build` (Vite) at `dist/overlay.js` — so a frontend
/// build must run before this crate compiles.
const OVERLAY_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../dist/overlay.js"
));

/// Returns the X-App data-directory rooted at `--config-base-dir`.
pub fn x_app_data_dir<R: Runtime>(handle: &AppHandle<R>) -> std::path::PathBuf {
    let args = handle.state::<Args>();
    args.config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join("x-app")
}

/// Build the X-App window and its two child webviews if they don't
/// already exist. Idempotent.
pub fn create_x_app<R: Runtime>(handle: &AppHandle<R>) -> tauri::Result<()> {
    if handle.get_window(X_APP_WINDOW).is_some() {
        return Ok(());
    }

    let data_dir = x_app_data_dir(handle);
    std::fs::create_dir_all(&data_dir)?;

    // 1. Bare window — no auto-attached webview (we add children below).
    let window = WindowBuilder::new(handle, X_APP_WINDOW)
        .title("psychological-operations-browser — X-App")
        .inner_size(DEFAULT_WIDTH as f64, DEFAULT_HEIGHT as f64)
        .build()?;

    // 2. Panel webview on top: local Vite-built page.
    let panel = WebviewBuilder::new(
        PANEL_LABEL,
        WebviewUrl::App("panel.html".into()),
    );
    window.add_child(
        panel,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(DEFAULT_WIDTH as f64, PANEL_HEIGHT as f64),
    )?;

    // 3. Content webview below: remote console.x.ai + injected overlay.
    let url = Url::parse("https://console.x.com/").expect("hardcoded URL parses");
    let kick = handle.state::<WatcherKick>().0.clone();
    let content = WebviewBuilder::new(CONTENT_LABEL, WebviewUrl::External(url))
        .data_directory(data_dir)
        .initialization_script(OVERLAY_JS)
        .on_page_load(move |_webview, payload| {
            // On every page navigation, kick the sign-in watcher to
            // re-check cookies — catches sign-in / sign-out flips
            // immediately, before WebView2's lazy cookie disk flush.
            if matches!(payload.event(), PageLoadEvent::Finished) {
                kick.notify_one();
            }
        });
    window.add_child(
        content,
        LogicalPosition::new(0.0, PANEL_HEIGHT as f64),
        LogicalSize::new(
            DEFAULT_WIDTH as f64,
            (DEFAULT_HEIGHT - PANEL_HEIGHT) as f64,
        ),
    )?;

    // 4. On window resize, reflow both webviews to follow.
    let window_for_resize = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Resized(size) = event {
            reflow_physical(&window_for_resize, size.width, size.height);
        }
    });

    Ok(())
}

/// Resize the panel + content webviews based on the derived
/// [`PanelState`]. Called from the window-resize callback AND from
/// [`crate::state::recompute_and_publish`] on every state flip.
///
/// Panel is visible (height [`PANEL_HEIGHT`]) when the derivation
/// has something to show, hidden (height 0) otherwise. Before any
/// derivation has run (e.g. process startup, no mode set), the
/// panel is hidden — `state::current_panel()` returns `None`.
///
/// `width` / `height` are in physical pixels (window's `inner_size`).
pub fn reflow_physical<R: Runtime>(window: &Window<R>, width: u32, height: u32) {
    let visible = state::current_panel().is_some_and(|s| s.is_visible());
    let panel_h = if visible { PANEL_HEIGHT } else { 0 };

    if let Some(panel) = window.get_webview(PANEL_LABEL) {
        let _ = panel.set_size(PhysicalSize::new(width, panel_h));
    }
    if let Some(content) = window.get_webview(CONTENT_LABEL) {
        let _ = content.set_position(tauri::PhysicalPosition::new(0, panel_h as i32));
        let _ = content.set_size(PhysicalSize::new(width, height.saturating_sub(panel_h)));
    }
}

/// Reflow using the current `inner_size` of the X-App window.
/// Convenience wrapper used by [`crate::signin_watcher`] which
/// doesn't track the current size.
pub fn reflow<R: Runtime>(handle: &AppHandle<R>) {
    let Some(window) = handle.get_window(X_APP_WINDOW) else {
        return;
    };
    let size = match window.inner_size() {
        Ok(s) => s,
        Err(_) => return,
    };
    reflow_physical(&window, size.width, size.height);
}
