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
//!     Window on Linux) below the panel. CEF replaces the prior
//!     Tauri-managed WebView2 content webview — X gates login on
//!     WebView2-specific headers, so the content surface has to be
//!     real Chromium.
//!
//! The CEF browser is scoped to a per-mode `RequestContext`
//! (isolated cookies / cache). Mode switches go through
//! [`recreate_cef_content`]: close current browser, wait for
//! `LifeSpan::on_before_close`, open a new browser with the new
//! mode's RequestContext + start URL.

use std::sync::{Mutex, OnceLock};

use psychological_operations_browser_sdk::mode::Mode;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::webview::WebviewBuilder;
use tauri::window::WindowBuilder;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalSize, WebviewUrl, Window,
    WindowEvent, Wry,
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

/// Stashed parent-window handle (HWND/NSView/X11Window) of the
/// Tauri Window so [`recreate_cef_content`] can re-embed without
/// re-deriving from the live window each time. Set in
/// [`create_x_app`]; never cleared (the Tauri window outlives any
/// CEF browser).
static PARENT_HANDLE: OnceLock<Mutex<Option<isize>>> = OnceLock::new();

fn parent_handle_slot() -> &'static Mutex<Option<isize>> {
    PARENT_HANDLE.get_or_init(|| Mutex::new(None))
}

/// Returns the data-directory for the given mode rooted at
/// `--config-base-dir`. Mirrors the structure of CEF's per-mode
/// cache subdir so credentials live alongside the browser profile.
///
///   - X-App: `<config>/.../browser/x-app/`
///   - Psyop "foo": `<config>/.../browser/psyop/foo/`
pub fn mode_data_dir(handle: &AppHandle<Wry>, mode: &Mode) -> std::path::PathBuf {
    let base = handle
        .state::<Args>()
        .config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser");
    match mode {
        Mode::XApp => base.join("x-app"),
        Mode::PsyopRead { name } | Mode::PsyopAuthorize { name } => {
            base.join("psyop").join(name)
        }
    }
}

/// The shared `root_cache_path` passed to `cef::initialize`.
/// Per-mode caches live in subdirectories — see
/// [`cache_subdir_for`].
pub fn cef_root_cache_dir(handle: &AppHandle<Wry>) -> std::path::PathBuf {
    handle
        .state::<Args>()
        .config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join("cef-root")
}

/// Per-mode CEF cache subdirectory (relative to the cache root).
/// Returned as a string because CEF's `RequestContextSettings.cache_path`
/// takes a string path.
fn cache_subdir_for(mode: &Mode) -> String {
    match mode {
        Mode::XApp => "x-app".to_string(),
        // Flat single-segment subdir — CEF's Chrome runtime rejects
        // nested profile paths (`psyop/<name>`) with "Cannot create
        // profile at path …" and silently falls back to an in-memory
        // profile, breaking cookie persistence. `psyop-<name>` keeps
        // psyops under a flat namespace just like `x-app`.
        Mode::PsyopRead { name } | Mode::PsyopAuthorize { name } => {
            format!("psyop-{name}")
        }
    }
}

/// Initial URL each mode lands on.
fn start_url_for(mode: &Mode) -> &'static str {
    match mode {
        Mode::XApp => "https://console.x.com/",
        Mode::PsyopRead { .. } | Mode::PsyopAuthorize { .. } => "https://x.com/",
    }
}

/// Build the X-App window with its panel webview + the initial
/// CEF content surface for `mode`. Idempotent on the window
/// (re-creating is a no-op); the CEF surface is created exactly
/// once here. Use [`recreate_cef_content`] to switch modes later.
pub fn create_x_app(handle: &AppHandle<Wry>, mode: &Mode) -> tauri::Result<()> {
    if handle.get_window(X_APP_WINDOW).is_some() {
        return Ok(());
    }

    let data_dir = mode_data_dir(handle, mode);
    std::fs::create_dir_all(&data_dir)?;

    // 1. Bare window — no auto-attached webview (we add the panel
    //    + CEF surface ourselves below).
    let window = WindowBuilder::new(handle, X_APP_WINDOW)
        .title("psychological-operations-browser")
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

    // 3. CEF init: shared root cache. Per-mode RequestContexts
    //    branch out underneath at create_browser time.
    cef_embed::initialize(&cef_root_cache_dir(handle), handle.clone());

    let raw_parent = raw_parent_handle(&window);
    if let Ok(mut slot) = parent_handle_slot().lock() {
        *slot = Some(raw_parent);
    }

    // 4. First CEF browser, scoped to the initial mode.
    let (x, y, w, h) = cef_bounds(&window);
    cef_embed::create_browser(raw_parent, x, y, w, h, &cache_subdir_for(mode), start_url_for(mode));

    // 5. On window resize, reflow the panel + reposition the CEF
    //    child surface. On close, ask CEF to close the browser BEFORE
    //    Tauri tears the parent surface down.
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

/// Tear down the current CEF browser and open a new one with
/// `mode`'s RequestContext + start URL. Called from
/// [`crate::stdio`] when a mode-switch stdin request lands.
///
/// Synchronous from the caller's perspective: blocks (with a
/// timeout) on `LifeSpan::on_before_close` so the new browser
/// doesn't race against the old one's teardown. Caller (stdio
/// reader thread) further waits on the new overlay's
/// `frontend_ready` invoke to ensure the JS side is up before
/// processing more stdin lines.
pub fn recreate_cef_content(handle: &AppHandle<Wry>, mode: &Mode) {
    // Close the current browser if any; wait for on_before_close.
    if cef_embed::has_browser() {
        let close_rx = cef_embed::install_close_signal();
        cef_embed::close_browser_async();
        let _ = close_rx.recv_timeout(std::time::Duration::from_secs(10));
    }

    let Some(parent) = parent_handle_slot().lock().ok().and_then(|s| *s) else {
        return;
    };
    let Some(window) = handle.get_window(X_APP_WINDOW) else {
        return;
    };
    let (x, y, w, h) = cef_bounds(&window);
    cef_embed::create_browser(parent, x, y, w, h, &cache_subdir_for(mode), start_url_for(mode));
}

/// Compute the CEF child surface's bounds inside the Tauri
/// window — full width, full height minus the panel strip.
fn cef_bounds(window: &Window<Wry>) -> (i32, i32, i32, i32) {
    let size = window
        .inner_size()
        .unwrap_or(PhysicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));
    let panel_h_phys = panel_height_physical(window);
    let x = 0_i32;
    let y = panel_h_phys as i32;
    let w = size.width as i32;
    let h = (size.height as i32 - panel_h_phys as i32).max(1);
    (x, y, w, h)
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
fn raw_parent_handle(window: &Window<Wry>) -> isize {
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
fn panel_height_physical(window: &Window<Wry>) -> u32 {
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
pub fn reflow_physical(window: &Window<Wry>, width: u32, height: u32) {
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
pub fn reflow(handle: &AppHandle<Wry>) {
    let Some(window) = handle.get_window(X_APP_WINDOW) else {
        return;
    };
    let size = match window.inner_size() {
        Ok(s) => s,
        Err(_) => return,
    };
    reflow_physical(&window, size.width, size.height);
}
