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
//! (isolated cookies / cache). Mode is locked at CLI args for
//! the lifetime of the process; the RequestContext is built
//! once in [`create_x_app`] and never rebuilt.

use psychological_operations_sdk::browser::mode::Mode;
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

/// The shared `root_cache_path` passed to `cef::initialize`.
/// Per-mode caches live in subdirectories — see
/// [`cache_subdir_for`].
pub fn cef_root_cache_dir(handle: &AppHandle<Wry>) -> std::path::PathBuf {
    handle
        .state::<Args>()
        .state_dir
        .join("browser")
        .join("cef-root")
}

/// Per-mode CEF cache subdirectory (relative to the cache root). Single
/// source of truth is [`Mode::cache_subdir`] — the db crate's cookie
/// probe keys off the same mapping, so they must not drift. Each persona
/// is ONE flat dir directly under `cef-root` (Chrome's runtime only
/// accepts a profile whose cache_path is an immediate child of the root).
fn cache_subdir_for(mode: &Mode) -> String {
    mode.cache_subdir()
}

/// Initial URL each mode lands on.
fn start_url_for(mode: &Mode) -> &'static str {
    match mode {
        Mode::XApp => "https://console.x.com/",
        Mode::AgentRead { .. }
        | Mode::AgentAuthorize { .. }
        | Mode::AgentBrowser { .. }
        | Mode::AgentDeliver { .. } => "https://x.com/",
    }
}

/// Build the window shell — bare window + panel webview + CEF init +
/// the resize/close-flush handler — WITHOUT a content browser. Shared by
/// [`create_x_app`] (persona modes) and [`create_deliver_window`]
/// (delivery). Returns `Some(window)` on first build, `None` if the
/// window already exists (idempotent).
fn build_shell(handle: &AppHandle<Wry>) -> tauri::Result<Option<Window<Wry>>> {
    if handle.get_window(X_APP_WINDOW).is_some() {
        return Ok(None);
    }

    // 1. Bare window — no auto-attached webview (we add the panel
    //    + CEF surface ourselves below).
    let window = WindowBuilder::new(handle, X_APP_WINDOW)
        .title("psychological-operations-browser")
        .inner_size(DEFAULT_WIDTH as f64, DEFAULT_HEIGHT as f64)
        .build()?;

    // 2. Panel webview on top: local Vite-built page.
    let panel = WebviewBuilder::new(PANEL_LABEL, WebviewUrl::App("panel.html".into()));
    window.add_child(
        panel,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(DEFAULT_WIDTH as f64, PANEL_HEIGHT as f64),
    )?;

    // 3. CEF init: shared root cache. Per-context RequestContexts
    //    branch out underneath at create_browser time.
    cef_embed::initialize(&cef_root_cache_dir(handle), handle.clone());

    // 4. On window resize, reflow the panel + reposition the CEF
    //    child surface. On close, ask CEF to close the browser BEFORE
    //    Tauri tears the parent surface down (flush the cookie store).
    let window_for_event = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            reflow_physical(&window_for_event, size.width, size.height);
        }
        WindowEvent::CloseRequested { .. } => {
            // Block until CEF reports `on_before_close` (the in-memory
            // cookie store has been flushed to disk). Guard on
            // `has_browser` so we don't wait on an already-closed browser.
            if cef_embed::has_browser() {
                let close_rx = cef_embed::install_close_signal();
                cef_embed::close_browser_async();
                let _ = close_rx.recv_timeout(std::time::Duration::from_secs(5));
            }
        }
        _ => {}
    });

    Ok(Some(window))
}

/// Build the X-App window with its panel webview + the initial CEF
/// content surface for `mode`. Idempotent on the window.
pub fn create_x_app(handle: &AppHandle<Wry>, mode: &Mode) -> tauri::Result<()> {
    let Some(window) = build_shell(handle)? else {
        return Ok(());
    };
    // The single CEF browser for this process, scoped to the CLI-locked mode.
    let raw_parent = raw_parent_handle(&window);
    let (x, y, w, h) = cef_bounds(&window);
    cef_embed::create_browser(raw_parent, x, y, w, h, &cache_subdir_for(mode), start_url_for(mode));
    Ok(())
}

/// Build the window shell for **delivery** mode — no content browser is
/// created here; the deliver driver creates one CEF browser per agent
/// (each scoped to that agent's `cef-root/agent-<tag>/` profile) via
/// [`spawn_agent_browser`].
pub fn create_deliver_window(handle: &AppHandle<Wry>) -> tauri::Result<()> {
    build_shell(handle)?;
    Ok(())
}

/// Create a CEF content browser scoped to `cache_subdir`, sized to the
/// window's content area, loading `url`. Used by the deliver driver to
/// open each agent's session in turn. No-op if the window isn't built.
pub fn spawn_agent_browser(handle: &AppHandle<Wry>, cache_subdir: &str, url: &str) {
    let Some(window) = handle.get_window(X_APP_WINDOW) else {
        return;
    };
    let raw_parent = raw_parent_handle(&window);
    let (x, y, w, h) = cef_bounds(&window);
    cef_embed::create_browser(raw_parent, x, y, w, h, cache_subdir, url);
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
