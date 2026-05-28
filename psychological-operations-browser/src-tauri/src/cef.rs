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
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

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

    // Client carries the LifeSpanHandler (Phase 3); Phase 4 will
    // extend it with LoadHandler + DisplayHandler.
    let mut client = ContentClient::new();
    let _ = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
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

// ---------------------------------------------------------------------
// CEF Client + LifeSpanHandler (Phase 3)
// ---------------------------------------------------------------------
//
// `ContentClient` is the single `Client` we pass to every
// `browser_host_create_browser` call. It composes the per-aspect CEF
// handlers (LifeSpan today, Load + Display + Scheme registration in
// Phase 4).
//
// LifeSpan keeps two pieces of state alive at module scope:
//   - `BROWSER`: the `Browser` handle (refcounted; cheaply Clone'd
//     on capture). Phase 4's `cef::navigate` and Phase 3's
//     `close_browser_async` use this to drive the embedded browser
//     from outside the LifeSpan callback.
//   - `BROWSER_CHILD_HWND`: the platform window handle of the
//     embedded CEF surface. Used by `set_browser_bounds` to
//     `SetWindowPos` the CEF area on every reflow.
//
// Both are cleared in `on_before_close` so post-close code paths
// don't try to act on a dead browser.

static BROWSER: OnceLock<Mutex<Option<Browser>>> = OnceLock::new();
static BROWSER_CHILD_HWND: OnceLock<Mutex<Option<isize>>> = OnceLock::new();

fn browser_slot() -> &'static Mutex<Option<Browser>> {
    BROWSER.get_or_init(|| Mutex::new(None))
}

fn browser_child_hwnd_slot() -> &'static Mutex<Option<isize>> {
    BROWSER_CHILD_HWND.get_or_init(|| Mutex::new(None))
}

wrap_life_span_handler! {
    struct LifeSpan {}

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(b) = browser else { return };
            // Capture child HWND/NSView/X11Window for reflow.
            if let Some(host) = b.host() {
                let raw = raw_from_handle(host.window_handle());
                if let Ok(mut slot) = browser_child_hwnd_slot().lock() {
                    *slot = Some(raw);
                }
            }
            // Capture the Browser itself for close + Phase 4 navigate.
            // Clone bumps the CEF refcount so the handle outlives the
            // borrow we got here.
            let cloned = b.clone();
            if let Ok(mut slot) = browser_slot().lock() {
                *slot = Some(cloned);
            }
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            // 0 = allow the close to proceed normally. We have no
            // pre-close UI work to interleave.
            0
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if let Ok(mut slot) = browser_slot().lock() { *slot = None; }
            if let Ok(mut slot) = browser_child_hwnd_slot().lock() { *slot = None; }
        }
    }
}

wrap_client! {
    pub struct ContentClient {}

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(LifeSpan::new())
        }
    }
}

/// Inverse of [`handle_from_raw`]: extract the platform-native raw
/// integer/pointer from a `cef_window_handle_t`.
#[cfg(target_os = "windows")]
fn raw_from_handle(h: cef_window_handle_t) -> isize {
    h.0 as isize
}

#[cfg(target_os = "macos")]
fn raw_from_handle(h: cef_window_handle_t) -> isize {
    h as isize
}

#[cfg(target_os = "linux")]
fn raw_from_handle(h: cef_window_handle_t) -> isize {
    h as isize
}

/// Resize the embedded CEF browser child window to fill the given
/// region inside its parent. No-op if no browser exists yet.
///
/// Coordinates are physical pixels relative to the parent.
///
/// Windows: `SetWindowPos` directly on the captured child HWND.
/// macOS / Linux: TODO — `setFrame` / `XMoveResizeWindow` need
/// platform-specific dependencies we haven't pulled in yet
/// (objc2-foundation NSRect, x11-dl for the display). Until that
/// lands, CEF on those platforms keeps its initial bounds.
pub fn set_browser_bounds(x: i32, y: i32, width: i32, height: i32) {
    let Some(child) = browser_child_hwnd_slot()
        .lock()
        .ok()
        .and_then(|s| *s)
    else {
        return;
    };

    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
        };
        let hwnd: HWND = child as HWND;
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (child, x, y, width, height);
    }
}

wrap_task! {
    struct CloseBrowserTask {}

    impl Task {
        fn execute(&self) {
            let browser = browser_slot()
                .lock()
                .ok()
                .and_then(|s| s.as_ref().cloned());
            let Some(b) = browser else { return };
            let Some(host) = b.host() else { return };
            host.close_browser(0); // 0 = graceful (fires onbeforeunload)
        }
    }
}

/// Ask CEF to close the embedded browser. Fire-and-forget — CEF
/// posts the close to its UI thread and runs `LifeSpanHandler::do_close`
/// → `on_before_close` asynchronously. Called from the Tauri window's
/// `CloseRequested` handler so the close starts BEFORE the parent
/// surface is destroyed.
pub fn close_browser_async() {
    if !is_initialized() {
        return;
    }
    let mut task = CloseBrowserTask::new();
    post_task(ThreadId::UI, Some(&mut task));
}

// ---------------------------------------------------------------------
// Cookie reading (Phase 2)
// ---------------------------------------------------------------------
//
// `snapshot_cookies(url)` reads every cookie CEF has for the given URL
// and returns them as a `Vec<(name, value)>`. Synchronous interface
// over CEF's callback-based `CookieManager::visit_url_cookies`.
//
// Threading: CEF requires `visit_url_cookies` to be called on the CEF
// UI thread, and the visitor's callbacks fire there too. We
// [`post_task`] the work to that thread and `recv_timeout` on a
// `sync_channel(1)` for the result. Caller blocks on a worker tokio
// `spawn_blocking` task — never the main thread.
//
// Completion signalling: CEF calls `visit` once per cookie with
// `count` (0-indexed) and `total` parameters. We detect the final
// cookie via `count + 1 >= total` and `take()` the channel sender
// from an `Arc<Mutex<Option<Sender>>>` so only that call delivers.
// Zero-cookies case (visitor never fires) → caller times out after
// 5 s and gets an empty Vec. Acceptable: the next `WatcherKick`
// re-snapshots, and pre-signin x.com normally has > 0 cookies anyway.

type CookieTx = Arc<Mutex<Option<SyncSender<Vec<(String, String)>>>>>;

wrap_cookie_visitor! {
    struct CollectingVisitor {
        collected: Arc<Mutex<Vec<(String, String)>>>,
        tx: CookieTx,
    }

    impl CookieVisitor {
        fn visit(
            &self,
            cookie: Option<&Cookie>,
            count: i32,
            total: i32,
            _delete_cookie: Option<&mut i32>,
        ) -> i32 {
            if let Some(c) = cookie {
                let name = c.name.to_string();
                let value = c.value.to_string();
                if let Ok(mut v) = self.collected.lock() {
                    v.push((name, value));
                }
            }
            // Last cookie? Deliver.
            if count + 1 >= total {
                let result = self
                    .collected
                    .lock()
                    .map(|v| v.clone())
                    .unwrap_or_default();
                if let Ok(mut g) = self.tx.lock() {
                    if let Some(s) = g.take() {
                        let _ = s.send(result);
                    }
                }
            }
            1 // continue iteration
        }
    }
}

wrap_task! {
    struct SnapshotCookiesTask {
        url: String,
        tx: CookieTx,
    }

    impl Task {
        fn execute(&self) {
            let Some(manager) = cookie_manager_get_global_manager(None) else {
                // No global cookie manager (CEF context not ready?).
                if let Ok(mut g) = self.tx.lock() {
                    if let Some(s) = g.take() { let _ = s.send(Vec::new()); }
                }
                return;
            };
            let collected = Arc::new(Mutex::new(Vec::new()));
            let mut visitor = CollectingVisitor::new(collected, self.tx.clone());
            let url = CefString::from(self.url.as_str());
            let accepted = manager.visit_url_cookies(Some(&url), 1, Some(&mut visitor));
            if accepted != 1 {
                // Visitor rejected — deliver empty so caller doesn't block.
                if let Ok(mut g) = self.tx.lock() {
                    if let Some(s) = g.take() { let _ = s.send(Vec::new()); }
                }
            }
            // Otherwise visitor's visit() will eventually fire and deliver.
        }
    }
}

/// Snapshot every cookie CEF holds for `url`, blocking up to 5
/// seconds. Returns an empty Vec if CEF isn't initialized, if the
/// task can't be posted, or if the request times out (e.g. zero
/// cookies → visitor never fires).
///
/// Must NOT be called from CEF's UI thread (would deadlock on the
/// sync_channel `recv`). Call from a Tokio `spawn_blocking` worker.
pub fn snapshot_cookies(url: &str) -> Vec<(String, String)> {
    if !is_initialized() {
        return Vec::new();
    }

    let (tx, rx) = sync_channel::<Vec<(String, String)>>(1);
    let tx: CookieTx = Arc::new(Mutex::new(Some(tx)));

    let mut task = SnapshotCookiesTask::new(url.to_string(), tx);
    let posted = post_task(ThreadId::UI, Some(&mut task));
    if posted != 1 {
        return Vec::new();
    }

    rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
}
