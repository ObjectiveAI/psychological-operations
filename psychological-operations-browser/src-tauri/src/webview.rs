//! Tauri window + child surface construction.
//!
//! One Tauri `Window` (label `x-app`) with:
//!
//!   - A Tauri child `Webview` ([`PANEL_LABEL`]) at the top, sized
//!     to [`PANEL_HEIGHT`] when the derived
//!     [`crate::state::PanelState`] is `Show`, 0 when `Hidden` (or
//!     before any derivation has run). Loads our local Vite-built
//!     `panel.html` (tauri:// asset protocol) so Tauri IPC works
//!     unconditionally — no remote-URL ACL needed.
//!
//!   - A CEF (Chromium Embedded Framework) browser embedded as a
//!     native child surface (HWND on Windows, NSView on macOS, X11
//!     Window on Linux) below the panel, loading
//!     `https://console.x.com/`. CEF replaces the prior
//!     Tauri-managed WebView2 content webview — X gates login on
//!     WebView2-specific headers, so the content surface has to be
//!     real Chromium.
//!
//! Layout reflow happens (a) on window resize and (b) on every
//! derived-state flip via [`reflow`], called from
//! [`crate::state::recompute_and_publish`]. Only the panel's size
//! is updated here today; the CEF browser tracks its parent
//! HWND/NSView/Window sizing automatically until Phase 5 wires
//! [`crate::cef::set_browser_bounds`].

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::webview::WebviewBuilder;
use tauri::window::WindowBuilder;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalSize, Runtime, WebviewUrl,
    Window, WindowEvent,
};

use crate::args::Args;
use crate::cef as cef_embed;
use crate::state;

/// Label of the single Tauri Window the X-App lives in.
pub const X_APP_WINDOW: &str = "x-app";

/// Label of the panel child webview (top of the window).
pub const PANEL_LABEL: &str = "panel";

/// Logical pixel height of the panel webview when visible.
const PANEL_HEIGHT: u32 = 48;
const DEFAULT_WIDTH: u32 = 1200;
const DEFAULT_HEIGHT: u32 = 800;

/// Returns the X-App data-directory rooted at `--config-base-dir`.
pub fn x_app_data_dir<R: Runtime>(handle: &AppHandle<R>) -> std::path::PathBuf {
    let args = handle.state::<Args>();
    args.config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join("x-app")
}

/// Build the X-App window with its panel webview and CEF content
/// surface if they don't already exist. Idempotent.
pub fn create_x_app<R: Runtime>(handle: &AppHandle<R>) -> tauri::Result<()> {
    if handle.get_window(X_APP_WINDOW).is_some() {
        return Ok(());
    }

    let data_dir = x_app_data_dir(handle);
    std::fs::create_dir_all(&data_dir)?;

    // 1. Bare window — no auto-attached webview (we add the panel
    //    + CEF surface ourselves below).
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

    // 3. CEF browser as native child surface below the panel.
    //    Per-mode CEF cache lives under `<data_dir>/cef/`; for the
    //    X-App that's `<config-base-dir>/.../x-app/cef/`. A future
    //    psyop mode would pass its own data dir the same way.
    cef_embed::initialize(&data_dir.join("cef"));

    let raw_parent = raw_parent_handle(&window);
    let size = window
        .inner_size()
        .unwrap_or(PhysicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));
    let panel_h_phys = panel_height_physical(&window);
    let x = 0_i32;
    let y = panel_h_phys as i32;
    let w = size.width as i32;
    let h = (size.height as i32 - panel_h_phys as i32).max(1);
    cef_embed::create_browser(raw_parent, x, y, w, h);

    // 4. On window resize, reflow the panel + reposition the CEF
    //    child surface. On close, ask CEF to close the browser BEFORE
    //    Tauri tears the parent surface down — gives CEF time to run
    //    `LifeSpanHandler::on_before_close` and reap subprocesses.
    let window_for_event = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            reflow_physical(&window_for_event, size.width, size.height);
        }
        WindowEvent::CloseRequested { .. } => {
            cef_embed::close_browser_async();
        }
        _ => {}
    });

    Ok(())
}

/// Extract the platform-native parent handle (HWND / NSView / X11
/// Window) from a Tauri window, as an `isize` for downstream
/// platform-specific casts in [`crate::cef`].
///
/// Panics if the window handle is unavailable (a programming error
/// — the window was just built) or if the variant isn't one CEF's
/// child-window embed supports. Wayland is the notable gap: CEF's
/// embed model is X11/HWND/NSView shaped, and the cef-rs crate's
/// `cef_window_handle_t` doesn't carry a Wayland surface. Linux
/// Wayland users should launch with `GDK_BACKEND=x11` so GTK gives
/// us an XWayland X11 window.
fn raw_parent_handle<R: Runtime>(window: &Window<R>) -> isize {
    let handle = window.window_handle().expect("window_handle failed");
    match handle.as_raw() {
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(h) => h.hwnd.get(),
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr() as isize,
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(h) => h.window as isize,
        other => panic!(
            "CEF backend doesn't support this window handle variant: {other:?}"
        ),
    }
}

/// Panel height in physical pixels (it's defined in logical pixels;
/// scale by the window's current scale factor).
fn panel_height_physical<R: Runtime>(window: &Window<R>) -> u32 {
    let scale = window.scale_factor().unwrap_or(1.0);
    (PANEL_HEIGHT as f64 * scale).round() as u32
}

/// Resize the panel webview AND reposition the CEF content surface
/// based on the derived [`PanelState`]. Called from the
/// window-resize callback AND from
/// [`crate::state::recompute_and_publish`] on every state flip.
///
/// Panel is visible (logical height [`PANEL_HEIGHT`]) when the
/// derivation has something to show, hidden (height 0) otherwise.
/// Before any derivation has run (e.g. process startup, no mode
/// set), the panel is hidden — `state::current_panel()` returns
/// `None`.
///
/// `width` / `height` are in physical pixels (window's `inner_size`).
/// The CEF browser is repositioned via [`crate::cef::set_browser_bounds`]
/// → platform `SetWindowPos` / equivalent.
pub fn reflow_physical<R: Runtime>(window: &Window<R>, width: u32, height: u32) {
    let visible = state::current_panel().is_some_and(|s| s.is_visible());
    let panel_h_logical = if visible { PANEL_HEIGHT } else { 0 };

    // Scale the panel-height to physical for the CEF reposition.
    let scale = window.scale_factor().unwrap_or(1.0);
    let panel_h_phys = (panel_h_logical as f64 * scale).round() as u32;

    if let Some(panel) = window.get_webview(PANEL_LABEL) {
        // Tauri's set_size on a child webview takes physical pixels.
        let _ = panel.set_size(PhysicalSize::new(width, panel_h_phys));
    }
    cef_embed::set_browser_bounds(
        0,
        panel_h_phys as i32,
        width as i32,
        (height as i32 - panel_h_phys as i32).max(1),
    );
}

/// Reflow using the current `inner_size` of the X-App window.
/// Convenience wrapper used by [`crate::state::recompute_and_publish`]
/// which doesn't track the current size.
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
