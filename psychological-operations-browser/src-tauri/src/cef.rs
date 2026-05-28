//! CEF (Chromium Embedded Framework) child-window embed.
//!
//! Why CEF? X gates login on something WebView2-specific (most
//! likely the `Microsoft Edge WebView2` brand in the `Sec-CH-UA`
//! header, which isn't honestly overridable). CEF is a real
//! Chromium build with no WebView2 brand, embedded as a native
//! child surface inside the existing Tauri window. The panel
//! webview (top) stays Tauri/WebView2 (or WKWebView on macOS,
//! WebKitGTK on Linux) — it's local (tauri://) and never touches
//! x.com.
//!
//! Process model:
//!   - On Windows/Linux the same binary is reused as CEF helper
//!     subprocesses (renderer, GPU, utility, ...). Chromium passes
//!     `--type=...` on the command line. [`is_helper_process`]
//!     checks for that before clap runs; if true,
//!     [`run_helper_and_exit`] hands off to CEF and terminates.
//!   - On macOS, helpers are SEPARATE executables that live inside
//!     `.app/Contents/Frameworks/<helper>.app/Contents/MacOS/`. The
//!     `bundle-cef-app` tool from `cef-rs` builds these. Our main
//!     binary never sees `--type=` — the OS launches helpers from
//!     their own bundle, so [`is_helper_process`] always returns
//!     false on macOS.
//!   - In the browser process, [`initialize`] is called once at
//!     startup BEFORE the Tauri builder. Configures CEF with
//!     `multi_threaded_message_loop = 1` so CEF spawns its own UI
//!     thread instead of taking over the main thread — leaves the
//!     main thread free for Tauri's wry/tao event loop.
//!     `multi_threaded_message_loop` is a Windows/Linux feature
//!     only; on macOS we use `external_message_pump` instead (TODO
//!     for the macOS bundling pass).
//!
//! Threading: CEF browser creation must happen on CEF's UI thread.
//! For Phase 1 we call `browser_host_create_browser` directly from
//! the Tauri main thread — CEF marshals internally. If that proves
//! flaky we'll route through `post_task(ThreadId::UI, ...)`.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use cef::sys::cef_window_handle_t;
use cef::*;

/// Initial URL the CEF browser loads when first created. Same target
/// the WebView2 backend uses today.
const INITIAL_URL: &str = "https://console.x.com/";

/// Stashed parent-window handle of the embedded CEF browser as a
/// raw `isize`. On Windows this is an HWND, on macOS an NSView
/// pointer, on Linux an X11 Window id. Kept here so the resize
/// callback can pass it back into CEF without holding a CEF handle
/// across threads.
static PARENT_HANDLE: OnceLock<Mutex<Option<isize>>> = OnceLock::new();

/// Guards [`initialize`] so callers can invoke it idempotently from
/// per-mode setup paths (X-App creation, future psyop creation). The
/// first call wins; subsequent calls assert the cache_root matches
/// what we already initialized with.
static INITIALIZED_WITH: OnceLock<std::path::PathBuf> = OnceLock::new();

/// On macOS the CEF library is loaded dynamically. We keep the
/// LibraryLoader alive for the lifetime of the process — dropping
/// it would unload the framework while CEF is still using it.
#[cfg(target_os = "macos")]
static MAC_LIBRARY: OnceLock<library_loader::LibraryLoader> = OnceLock::new();

fn parent_handle_slot() -> &'static Mutex<Option<isize>> {
    PARENT_HANDLE.get_or_init(|| Mutex::new(None))
}

/// Has [`initialize`] already been called?
pub fn is_initialized() -> bool {
    INITIALIZED_WITH.get().is_some()
}

/// Returns true iff the current process was spawned by Chromium as
/// a helper (renderer / GPU / utility / ...). Chromium passes the
/// helper kind via `--type=<kind>` on the command line.
///
/// Called BEFORE clap so the helper's switches don't trip clap's
/// "unknown argument" error.
///
/// On macOS this always returns false — helpers are separate .app
/// bundles, not invocations of the main binary.
pub fn is_helper_process() -> bool {
    #[cfg(target_os = "macos")]
    {
        return false;
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::args().any(|arg| arg.starts_with("--type="))
    }
}

/// Run as a CEF helper subprocess and exit. Must be called from
/// `main()` BEFORE Tauri starts, and only when [`is_helper_process`]
/// returned true.
///
/// `execute_process` returns ≥0 in helper processes (the helper has
/// finished its work); we then exit with code 0. This function never
/// returns.
///
/// Not defined on macOS — see module docs.
#[cfg(not(target_os = "macos"))]
pub fn run_helper_and_exit() -> ! {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let main_args = args.as_main_args();

    // Helper processes don't initialize CEF; `execute_process` runs
    // the helper to completion and returns.
    let _ret = execute_process(Some(main_args), None, std::ptr::null_mut());
    std::process::exit(0);
}

/// Initialize CEF in the browser process. Idempotent: the first
/// call wins, later calls assert the same `cache_root` was used.
///
/// `cache_root` is where Chromium puts its profile data (cookies,
/// local storage, cache). It is per-mode: for the X-App that's
/// `<config-base-dir>/plugins/.../x-app/cef/`; for a future psyop
/// it would be `<config-base-dir>/plugins/.../psyop/<name>/cef/`.
/// The caller (the per-mode webview-creation path in
/// [`crate::webview`]) computes the right one.
///
/// Safe to call from a Tauri `setup` closure — CEF's
/// `multi_threaded_message_loop` spawns its own UI thread, so the
/// main thread stays available for Tauri's event loop even after
/// init.
pub fn initialize(cache_root: &Path) {
    if let Some(existing) = INITIALIZED_WITH.get() {
        assert_eq!(
            existing.as_path(),
            cache_root,
            "cef::initialize called twice with different cache_roots — \
             only one CEF root is supported per process. Existing: {} \
             New: {}",
            existing.display(),
            cache_root.display()
        );
        return;
    }

    std::fs::create_dir_all(cache_root).ok();

    // macOS: load the CEF framework dynamically. Must happen BEFORE
    // any CEF API call. The loader is stored in a static so it lives
    // for the rest of the process.
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().expect("current_exe failed");
        let loader = library_loader::LibraryLoader::new(&exe, false);
        assert!(loader.load(), "failed to load CEF framework on macOS");
        let _ = MAC_LIBRARY.set(loader);
    }

    // CEF API version handshake. Must be called before any other CEF
    // function in the browser process.
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let cef_args = args::Args::new();
    let main_args = cef_args.as_main_args();

    // In the browser process `execute_process` returns -1 — it
    // recognizes that there's no `--type=` switch and bails so we
    // can proceed to `initialize`. Helper processes were screened
    // out earlier in `main` (Windows/Linux); calling this anyway is
    // the documented CEF entry point.
    let ret = execute_process(Some(main_args), None, std::ptr::null_mut());
    assert_eq!(
        ret, -1,
        "execute_process unexpectedly returned a non-browser-process value \
         (helper detection should have run before initialize)"
    );

    let root_cache_path = CefString::from(
        cache_root
            .to_str()
            .expect("cef cache_root must be valid UTF-8"),
    );

    // multi_threaded_message_loop is Windows/Linux only. On macOS we
    // fall back to CEF owning the main thread, which conflicts with
    // Tauri — proper macOS support requires external_message_pump
    // wired into Tauri's event loop (deferred to the macOS bundling
    // pass).
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let settings = Settings {
        multi_threaded_message_loop: 1,
        no_sandbox: 1,
        root_cache_path,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    let settings = Settings {
        no_sandbox: 1,
        root_cache_path,
        ..Default::default()
    };

    // `::cef::` is the absolute path to the extern crate (this
    // module is also named `cef`, which would shadow when reading
    // but Rust resolves to the extern crate anyway — the explicit
    // leading `::` makes that unambiguous to readers).
    let init_ret = ::cef::initialize(
        Some(main_args),
        Some(&settings),
        None, // no App handler for Phase 1 — defaults are fine
        std::ptr::null_mut(),
    );
    assert_eq!(init_ret, 1, "cef::initialize failed");

    let _ = INITIALIZED_WITH.set(cache_root.to_path_buf());
}

/// Tear CEF down after Tauri's event loop returns. Browsers must
/// already be closed (we hook the Tauri window close event in
/// `crate::webview` to call `BrowserHost::close_browser` before the
/// HWND/NSView is destroyed).
pub fn shutdown() {
    ::cef::shutdown();
}

/// Create a CEF browser embedded as a child surface of `parent`
/// at the given physical bounds, loading [`INITIAL_URL`].
///
/// `parent` is platform-specific: HWND on Windows, NSView pointer
/// on macOS, X11 Window id on Linux. The caller extracts it from
/// the Tauri window via raw-window-handle.
pub fn create_browser(parent: isize, x: i32, y: i32, width: i32, height: i32) {
    let bounds = Rect { x, y, width, height };
    unsafe { create_browser_inner(parent, bounds); }
}

unsafe fn create_browser_inner(parent: isize, bounds: Rect) {
    let cef_parent = handle_from_raw(parent);

    let window_info = WindowInfo {
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    }
    .set_as_child(cef_parent, &bounds);

    let url = CefString::from(INITIAL_URL);
    let settings = BrowserSettings::default();

    let _ = browser_host_create_browser(
        Some(&window_info),
        None, // Phase 1: no Client handlers yet (overlay/IPC come in Phase 3+)
        Some(&url),
        Some(&settings),
        None,
        None,
    );

    if let Ok(mut slot) = parent_handle_slot().lock() {
        *slot = Some(parent);
    }
}

/// Convert a raw platform handle (isize from raw-window-handle) into
/// the platform-specific `cef_window_handle_t` shape:
///   - Windows: `HWND` tuple struct wrapping `*mut c_void`. Since
///     `cef_window_handle_t` is a *type alias* for HWND (not a
///     newtype), we have to call the underlying HWND constructor —
///     `cef_window_handle_t(ptr)` doesn't parse because Rust won't
///     resolve a tuple-struct constructor through a type alias.
///   - macOS: raw `*mut c_void` (NSView pointer typedef).
///   - Linux: `c_ulong` integer (X11 Window typedef).
#[cfg(target_os = "windows")]
fn handle_from_raw(raw: isize) -> cef_window_handle_t {
    // `cef::sys::HWND` is cef-dll-sys's bindgen-generated tuple
    // struct `HWND(*mut HWND__)`. `cef_window_handle_t` is a type
    // alias for it, so we have to construct via the underlying
    // struct + cast the pointer to its opaque `HWND__` shape.
    cef::sys::HWND(raw as *mut cef::sys::HWND__)
}

#[cfg(target_os = "macos")]
fn handle_from_raw(raw: isize) -> cef_window_handle_t {
    raw as cef_window_handle_t
}

#[cfg(target_os = "linux")]
fn handle_from_raw(raw: isize) -> cef_window_handle_t {
    raw as cef_window_handle_t
}

/// Resize the embedded CEF browser child window to fill the given
/// region inside its parent. No-op if no browser exists yet.
///
/// Coordinates are physical pixels relative to the parent.
///
/// Phase 1 placeholder: actual repositioning requires a
/// LifeSpanHandler-captured browser handle so we can call the
/// platform `SetWindowPos`/`setFrame`/`XMoveResizeWindow` directly.
/// Until then CEF tracks the parent's resize events.
#[allow(dead_code, unused_variables)]
pub fn set_browser_bounds(x: i32, y: i32, width: i32, height: i32) {
    // TODO(phase 1.1): once we capture the browser's child handle in
    // on_after_created, reposition it directly via
    // SetWindowPos / setFrame / XMoveResizeWindow.
}
