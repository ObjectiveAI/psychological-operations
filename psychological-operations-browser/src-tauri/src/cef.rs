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
use tauri::{AppHandle, Manager, Wry};

use crate::WatcherKick;
use crate::state;

/// Self-contained IIFE bundle of the overlay. Produced by `yarn
/// build` (Vite) at `dist/overlay.js`, injected on every main-frame
/// load via [`InjectOverlay`].
const OVERLAY_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../dist/overlay.js"
));

/// Shared root cache path passed to `Settings.root_cache_path` at
/// [`initialize`] time. Immutable for the process lifetime (CEF
/// constraint). Per-account isolation comes from per-browser
/// [`RequestContext`]s whose `cache_path` is a subdirectory of
/// this root — see [`create_browser`].
static CACHE_ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Tauri `AppHandle` stashed at [`initialize`] time so handlers
/// running on CEF threads (`TrackUrl::on_address_change`,
/// `cef_scheme`'s command dispatcher) can reach into Tauri-managed
/// state. Concrete `Wry` runtime — the only runtime this binary
/// uses.
static APP_HANDLE: OnceLock<AppHandle<Wry>> = OnceLock::new();

pub fn app_handle() -> Option<&'static AppHandle<Wry>> {
    APP_HANDLE.get()
}

/// On macOS the CEF library is loaded dynamically. We keep the
/// LibraryLoader alive for the lifetime of the process — dropping
/// it would unload the framework while CEF is still using it.
#[cfg(target_os = "macos")]
static MAC_LIBRARY: OnceLock<library_loader::LibraryLoader> = OnceLock::new();

/// Has [`initialize`] already been called?
pub fn is_initialized() -> bool {
    CACHE_ROOT.get().is_some()
}

/// Convert a [`Path`] to a [`CefString`] suitable for handing to
/// CEF (`Settings.root_cache_path`, `Settings.log_file`,
/// `RequestContextSettings.cache_path`, etc.).
///
/// On Windows, Chromium's `chrome_browser_context.cc` rejects
/// paths with forward slashes — "Cannot create profile at path
/// ...". Rust's `PathBuf::join` preserves whatever separator was
/// already there, so a `--config-base-dir $USERPROFILE/.psyops`
/// CLI input (shell-expanded with `/`) joined to
/// `plugins/.../cef-root` (back-slashed) yields a mixed path
/// that fails CEF's validation. Normalize to all backslashes
/// here.
fn path_to_cef_string(path: &Path) -> CefString {
    let s = path
        .to_str()
        .expect("path must be valid UTF-8 for CEF");
    #[cfg(target_os = "windows")]
    {
        CefString::from(s.replace('/', "\\").as_str())
    }
    #[cfg(not(target_os = "windows"))]
    {
        CefString::from(s)
    }
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
/// We MUST pass an `App` here (not `None`) so the renderer process
/// sees `on_register_custom_schemes` and adds `psyops://` to its
/// own scheme registry. Without this, fetch() in the renderer
/// rejects the URL with "scheme not supported" even though the
/// browser-side scheme handler is registered.
///
/// Not defined on macOS — see module docs.
#[cfg(not(target_os = "macos"))]
pub fn run_helper_and_exit() -> ! {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let main_args = args.as_main_args();

    let mut app = ContentApp::new();
    let _ret = execute_process(Some(main_args), Some(&mut app), std::ptr::null_mut());
    std::process::exit(0);
}

/// Initialize CEF in the browser process. Idempotent: the first
/// call wins, later calls assert the same `cache_root` was used.
///
/// `cache_root` is the SHARED parent directory under which every
/// per-account `RequestContext` cache subdir lives. For this
/// binary that's
/// `<config-base-dir>/plugins/psychological-operations/browser/cef-root/`.
/// Per-account isolation comes from [`create_browser`]'s
/// `cache_subdir` argument — the cache root itself is process-
/// global and CAN NOT be changed after init (CEF's
/// `Settings.root_cache_path` is locked once set).
///
/// `app` is the Tauri `AppHandle` stashed for handlers that need
/// to reach back into Tauri state (URL tracking, the psyops://
/// scheme dispatcher).
///
/// Safe to call from a Tauri `setup` closure — CEF's
/// `multi_threaded_message_loop` spawns its own UI thread, so the
/// main thread stays available for Tauri's event loop even after
/// init.
pub fn initialize(cache_root: &Path, app: AppHandle<Wry>) {
    if let Some(existing) = CACHE_ROOT.get() {
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

    let root_cache_path = path_to_cef_string(cache_root);

    // Explicit log path so devs know where to look. CEF defaults
    // to writing `debug.log` somewhere under the CWD; pinning it
    // to `<cache_root>/cef-debug.log` keeps it next to the
    // profile and per-mode caches.
    let log_path = cache_root.join("cef-debug.log");
    let log_file = path_to_cef_string(&log_path);

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
        log_file,
        log_severity: LogSeverity::INFO,
        ..Default::default()
    };

    // macOS: NSApplication owns the main-thread message loop;
    // CEF can't take it over via `multi_threaded_message_loop`.
    // Use `external_message_pump` instead — CEF calls our
    // `BrowserProcessHandler::on_schedule_message_pump_work`
    // ([`PumpScheduler`]) which dispatches `cef::do_message_loop_work`
    // onto the Tauri main thread after the requested delay.
    //
    // The bundle paths point at the `.app` layout produced by
    // `bundle-cef-app` (via scripts/build-macos.sh):
    //   <App>.app/Contents/MacOS/<bin>             ← current_exe
    //   <App>.app/Contents/Frameworks/Chromium Embedded Framework.framework
    //   <App>.app/Contents/Frameworks/<helper>.app/Contents/MacOS/<helper>
    #[cfg(target_os = "macos")]
    let settings = {
        let exe = std::env::current_exe().expect("current_exe");
        let bundle = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .expect("current_exe not inside .app bundle layout");
        let frameworks = bundle.join("Contents").join("Frameworks");
        let framework = frameworks.join("Chromium Embedded Framework.framework");
        let helper_name = "psychological_operations_browser_helper";
        let helper_exe = frameworks
            .join(format!("{helper_name}.app"))
            .join("Contents")
            .join("MacOS")
            .join(helper_name);
        Settings {
            no_sandbox: 1,
            external_message_pump: 1,
            root_cache_path,
            log_file,
            log_severity: LogSeverity::INFO,
            main_bundle_path: CefString::from(
                bundle.to_str().expect("bundle path utf-8"),
            ),
            framework_dir_path: CefString::from(
                framework.to_str().expect("framework path utf-8"),
            ),
            browser_subprocess_path: CefString::from(
                helper_exe.to_str().expect("helper path utf-8"),
            ),
            ..Default::default()
        }
    };

    // `::cef::` is the absolute path to the extern crate (this
    // module is also named `cef`, which would shadow when reading
    // but Rust resolves to the extern crate anyway — the explicit
    // leading `::` makes that unambiguous to readers).
    let mut app_handler = ContentApp::new();
    let init_ret = ::cef::initialize(
        Some(main_args),
        Some(&settings),
        Some(&mut app_handler),
        std::ptr::null_mut(),
    );
    assert_eq!(init_ret, 1, "cef::initialize failed");

    // Register the `psyops://` scheme factory. Must run AFTER
    // initialize. Domain is None → matches any domain on the
    // scheme (`psyops://invoke/...`, `psyops://anything-else/...`
    // would all route through).
    let scheme_name = CefString::from("psyops");
    let mut factory = crate::cef_scheme::PsyopsFactory::new();
    let ok = register_scheme_handler_factory(Some(&scheme_name), None, Some(&mut factory));
    assert_eq!(ok, 1, "register_scheme_handler_factory(psyops) failed");

    let _ = CACHE_ROOT.set(cache_root.to_path_buf());
    let _ = APP_HANDLE.set(app);

    // Stdout breadcrumb so reviewers know where CEF puts its
    // profile + diagnostics. Crash dumps (if a subprocess
    // segfaults) land alongside the log file under `cache_root`.
    let _ = psychological_operations_browser_sdk::output::Output::Log {
        message: format!(
            "cef: initialized, cache_root={}, log_file={}",
            cache_root.display(),
            log_path.display(),
        ),
    }
    .emit();
}

/// Tear CEF down after Tauri's event loop returns. Browsers must
/// already be closed (we hook the Tauri window close event in
/// `crate::webview` to call `BrowserHost::close_browser` before the
/// HWND/NSView is destroyed).
pub fn shutdown() {
    ::cef::shutdown();
}

/// Create a CEF browser embedded as a child surface of `parent`
/// at the given physical bounds, loading `url` under a per-account
/// `RequestContext` whose `cache_path` lives at
/// `<cache_root>/<cache_subdir>/`.
///
/// `parent` is platform-specific: HWND on Windows, NSView pointer
/// on macOS, X11 Window id on Linux. The caller extracts it from
/// the Tauri window via raw-window-handle.
///
/// `cache_subdir` should be a relative path under the cache root —
/// e.g. `"x-app"` or `"psyop/foo"`. Each subdir = one isolated
/// cookie / localStorage / IndexedDB profile.
pub fn create_browser(
    parent: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    cache_subdir: &str,
    url: &str,
) {
    let bounds = Rect { x, y, width, height };
    unsafe { create_browser_inner(parent, bounds, cache_subdir, url); }
}

unsafe fn create_browser_inner(
    parent: isize,
    bounds: Rect,
    cache_subdir: &str,
    url: &str,
) {
    let cef_parent = handle_from_raw(parent);

    let window_info = WindowInfo {
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    }
    .set_as_child(cef_parent, &bounds);

    let url = CefString::from(url);
    let settings = BrowserSettings::default();

    // Per-account RequestContext. cache_path must be a subdir of
    // the root_cache_path passed at initialize() time, so CEF
    // accepts it as a sibling profile.
    let cache_root = CACHE_ROOT
        .get()
        .expect("cef::initialize must run before create_browser");
    let full_cache = cache_root.join(cache_subdir);
    std::fs::create_dir_all(&full_cache).ok();
    let ctx_settings = RequestContextSettings {
        cache_path: path_to_cef_string(&full_cache),
        persist_session_cookies: 1,
        ..Default::default()
    };
    let mut request_context = request_context_create_context(Some(&ctx_settings), None)
        .expect("request_context_create_context returned None");

    // Register the `psyops://` scheme handler factory on THIS
    // RequestContext. The free `register_scheme_handler_factory`
    // we call in `initialize()` only registers on the GLOBAL
    // context — our per-account contexts don't inherit it, so
    // overlay fetches would 404 against the factory registry of
    // this context. Register here so requests routed through
    // this RequestContext find the factory.
    let scheme_name = CefString::from("psyops");
    let mut factory = crate::cef_scheme::PsyopsFactory::new();
    request_context.register_scheme_handler_factory(
        Some(&scheme_name),
        None,
        Some(&mut factory),
    );

    // Client carries LifeSpan + Load + Display handlers.
    let mut client = ContentClient::new();
    let _ = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&settings),
        None,
        Some(&mut request_context),
    );

    let _ = parent;
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

/// One-shot channel that fires when `LifeSpan::on_before_close`
/// runs. [`install_close_signal`] populates the sender slot before
/// calling [`close_browser_async`]; [`stdio`]'s mode-switch waits
/// on the receiver to know the old browser is gone before opening
/// the new one.
static CLOSE_SIGNAL: OnceLock<Mutex<Option<SyncSender<()>>>> = OnceLock::new();

fn browser_slot() -> &'static Mutex<Option<Browser>> {
    BROWSER.get_or_init(|| Mutex::new(None))
}

fn browser_child_hwnd_slot() -> &'static Mutex<Option<isize>> {
    BROWSER_CHILD_HWND.get_or_init(|| Mutex::new(None))
}

fn close_signal_slot() -> &'static Mutex<Option<SyncSender<()>>> {
    CLOSE_SIGNAL.get_or_init(|| Mutex::new(None))
}

/// Install a fresh one-shot channel that fires when the next
/// `on_before_close` lands. Caller invokes this BEFORE
/// [`close_browser_async`] and `recv_timeout`s the returned
/// receiver to block until the old browser is fully gone (CEF
/// has cleared its child HWND, refcount dropped, etc.).
pub fn install_close_signal() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = sync_channel::<()>(1);
    *close_signal_slot().lock().expect("close signal slot poisoned") = Some(tx);
    rx
}

/// True iff there's a live browser captured in `BROWSER`
/// (i.e. `on_after_created` fired and `on_before_close` hasn't).
pub fn has_browser() -> bool {
    browser_slot()
        .lock()
        .map(|s| s.is_some())
        .unwrap_or(false)
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
            // Wake any caller blocked on `install_close_signal()`.
            if let Ok(mut g) = close_signal_slot().lock() {
                if let Some(tx) = g.take() {
                    let _ = tx.send(());
                }
            }
        }
    }
}

wrap_load_handler! {
    struct InjectOverlay {}

    impl LoadHandler {
        fn on_load_start(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _transition_type: TransitionType,
        ) {
            let Some(frame) = frame else {
                let _ = psychological_operations_browser_sdk::output::Output::Log {
                    message: "cef: on_load_start no frame".into(),
                }.emit();
                return;
            };
            let is_main = frame.is_main();
            // Only inject into the top-level frame — iframes inside
            // x.com (auth modals, etc.) shouldn't run our overlay.
            if is_main != 1 {
                return;
            }
            // Prepend the locked mode as a JS global so the
            // overlay can read it synchronously at startup
            // without round-tripping through an invoke.
            let mode_json = serde_json::to_string(
                &psychological_operations_browser_sdk::mode::get(),
            )
            .unwrap_or_else(|_| "null".into());
            let preamble = format!("window.__PSYOPS_MODE = {mode_json};\n");
            let _ = psychological_operations_browser_sdk::output::Output::Log {
                message: format!(
                    "cef: on_load_start main frame, injecting overlay ({} bytes)",
                    OVERLAY_JS.len()
                ),
            }
            .emit();
            let mut combined = String::with_capacity(preamble.len() + OVERLAY_JS.len());
            combined.push_str(&preamble);
            combined.push_str(OVERLAY_JS);
            let code = CefString::from(combined.as_str());
            // script_url + start_line are for DevTools stacks only.
            let script_url = CefString::from("psyops://overlay.js");
            frame.execute_java_script(Some(&code), Some(&script_url), 0);
        }
    }
}

wrap_display_handler! {
    struct TrackUrl {}

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            // Only main-frame URL changes matter for our state.
            let Some(f) = frame else { return };
            if f.is_main() != 1 { return; }
            let Some(u) = url else { return };
            let url_string = u.to_string();
            let Some(handle) = app_handle() else { return };
            // recompute_and_publish runs Tauri-thread-ish work
            // (emit_to, set_size, set_position); spawn to avoid
            // any potential CEF UI thread ↔ Tauri main thread
            // deadlock when both sides are busy.
            let handle_for_task = handle.clone();
            tauri::async_runtime::spawn(async move {
                state::set_current_url(&handle_for_task, url_string);
            });
            // Kick the cookies watcher — URL change often signals
            // a sign-in / sign-out / route flip.
            handle.state::<WatcherKick>().0.notify_one();
        }
    }
}

wrap_client! {
    pub struct ContentClient {}

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(LifeSpan::new())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(InjectOverlay::new())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(TrackUrl::new())
        }

        /// V8 native bridge response leg. The renderer's
        /// [`crate::cef_v8::OverlayV8Handler`] sends a
        /// `psyops_invoke` process message; we decode, dispatch
        /// via the shared [`crate::cef_scheme::dispatch_inner`],
        /// and ship the result back as
        /// `window.__psyops_recv(corrid, status, result_json)`
        /// via `execute_overlay_js`.
        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> i32 {
            let Some(message) = message else { return 0 };
            if !crate::cef_v8::is_invoke(message) {
                return 0;
            }
            let Some(envelope) = crate::cef_v8::parse_envelope(message) else {
                return 1;
            };
            let Some(app) = app_handle().cloned() else {
                return 1;
            };
            tauri::async_runtime::spawn(async move {
                let (status, result) = match crate::cef_scheme::dispatch_inner(
                    &app,
                    &envelope.cmd,
                    envelope.args.as_bytes(),
                )
                .await
                {
                    Ok(value) => (
                        "ok",
                        serde_json::to_string(&value)
                            .unwrap_or_else(|_| "null".into()),
                    ),
                    Err(e) => ("err", e.into_message()),
                };
                // window.__psyops_recv(corrid, status, result)
                let js = format!(
                    "window.__psyops_recv && window.__psyops_recv({corrid},{status},{result});",
                    corrid = envelope.corrid,
                    status = serde_json::to_string(status).unwrap_or_else(|_| "\"err\"".into()),
                    result = serde_json::to_string(&result).unwrap_or_else(|_| "\"\"".into()),
                );
                execute_overlay_js(js);
            });
            1
        }
    }
}

wrap_app! {
    pub struct ContentApp {}

    impl App {
        fn on_register_custom_schemes(
            &self,
            registrar: Option<&mut SchemeRegistrar>,
        ) {
            let Some(r) = registrar else { return };
            let name = CefString::from("psyops");
            // Standard: URL parses like a normal http(s) URL.
            // Secure: same-origin treats as a secure origin (fetch
            //   from x.com pages won't be blocked as mixed content).
            // CORS_ENABLED + FETCH_ENABLED: page-script fetch() can
            //   hit this scheme. Critical for our overlay's `invoke`
            //   helper that uses fetch("psyops://invoke/...").
            let options = SchemeOptions::STANDARD.get_raw()
                | SchemeOptions::SECURE.get_raw()
                | SchemeOptions::CORS_ENABLED.get_raw()
                | SchemeOptions::FETCH_ENABLED.get_raw();
            r.add_custom_scheme(Some(&name), options);
        }

        /// Renderer-side V8 binding installer. CEF calls this in
        /// every renderer subprocess (Windows/Linux: same exe
        /// re-entered with `--type=renderer`; macOS: the helper
        /// `.app` binary). The handler installs
        /// `window.__psyops_send` on every V8 context.
        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(crate::cef_v8::OverlayRenderProcessHandler::new())
        }

        // macOS only: hand CEF our message-pump scheduler so it
        // can drive `do_message_loop_work` calls on the main
        // thread (NSApplication owns the loop; we share).
        #[cfg(target_os = "macos")]
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(PumpScheduler::new())
        }
    }
}

/// macOS message-pump scheduler. CEF invokes
/// `on_schedule_message_pump_work(delay_ms)` whenever it has
/// pending work; we delay (via tokio sleep) then post a
/// `do_message_loop_work` call onto the Tauri main thread (which
/// IS the NSApplication main thread).
///
/// Defined on all platforms (the `wrap_browser_process_handler!`
/// macro produces a real struct, dead-code on non-macOS) so the
/// types resolve when ContentApp's method exists conditionally.
/// `#[cfg]` on the `default_client` here is just to mark the body
/// — the type itself is benign cross-platform.
#[cfg(target_os = "macos")]
wrap_browser_process_handler! {
    struct PumpScheduler {}

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            let Some(handle) = app_handle() else { return };
            let handle = handle.clone();
            let delay = delay_ms.max(0) as u64;
            tauri::async_runtime::spawn(async move {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                let _ = handle.run_on_main_thread(|| {
                    ::cef::do_message_loop_work();
                });
            });
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
///   - Windows: `SetWindowPos` on the captured child HWND.
///   - macOS: `[NSView setFrame:]` on the captured NSView* (CEF
///     internally compositions inside the view; setFrame triggers
///     the relayout).
///   - Linux: `XMoveResizeWindow` against CEF's shared X display.
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

    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::msg_send;
        use objc2_app_kit::NSView;
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let view = child as *mut NSView;
        if view.is_null() {
            return;
        }
        // AppKit coordinates: origin bottom-left of the parent
        // view's bounds, in points (logical pixels). Our caller
        // passes physical pixels and a top-left origin, so we'd
        // need to flip Y by the parent's height + divide by
        // backingScaleFactor for full correctness. For now we
        // pass the rect as-is — the parent NSView is the Tauri
        // window's content view which has flipped coordinates
        // when its `isFlipped` returns YES (Tauri's webview view
        // does). If isFlipped is NO we'll see an inverted layout
        // and tweak later.
        let frame = NSRect::new(
            NSPoint::new(x as f64, y as f64),
            NSSize::new(width as f64, height as f64),
        );
        let _: () = msg_send![&*view, setFrame: frame];
    }

    #[cfg(target_os = "linux")]
    unsafe {
        let xlib = match x11_dl::xlib::Xlib::open() {
            Ok(x) => x,
            Err(_) => return,
        };
        // Reuse CEF's already-open X display rather than opening
        // a second connection (and risking events being delivered
        // to the wrong queue).
        let display = ::cef::get_xdisplay();
        if display.is_null() {
            return;
        }
        (xlib.XMoveResizeWindow)(
            display as *mut _,
            child as x11_dl::xlib::Window,
            x,
            y,
            width as u32,
            height as u32,
        );
        (xlib.XFlush)(display as *mut _);
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

wrap_task! {
    struct ExecuteJsTask {
        code: String,
    }
    impl Task {
        fn execute(&self) {
            let browser = browser_slot()
                .lock()
                .ok()
                .and_then(|s| s.as_ref().cloned());
            let Some(b) = browser else { return };
            let Some(frame) = b.main_frame() else { return };
            let code = CefString::from(self.code.as_str());
            let script_url = CefString::from("psyops://stdio.js");
            frame.execute_java_script(Some(&code), Some(&script_url), 0);
        }
    }
}

/// Push a snippet of JavaScript into the main frame of the
/// embedded CEF browser. Fire-and-forget. Used by
/// [`crate::stdio::dispatch_request`] as the Rust → JS push channel
/// (replaces Tauri's `handle.emit("psyops:request", ...)`). The
/// overlay registers a global `window.__psyops.push(...)` handler
/// to receive the messages.
pub fn execute_overlay_js(code: impl Into<String>) {
    if !is_initialized() {
        return;
    }
    let mut task = ExecuteJsTask::new(code.into());
    post_task(ThreadId::UI, Some(&mut task));
}

wrap_task! {
    struct NavigateTask {
        url: String,
    }
    impl Task {
        fn execute(&self) {
            let browser = browser_slot()
                .lock()
                .ok()
                .and_then(|s| s.as_ref().cloned());
            let Some(b) = browser else { return };
            let Some(frame) = b.main_frame() else { return };
            let url = CefString::from(self.url.as_str());
            frame.load_url(Some(&url));
        }
    }
}

/// Navigate the embedded CEF browser to `url`. Fire-and-forget.
/// Used by [`crate::state`] post-sign-in to bounce back to
/// `console.x.com/` even if OAuth left us in some in-between
/// origin.
pub fn navigate(url: impl Into<String>) {
    if !is_initialized() {
        return;
    }
    let mut task = NavigateTask::new(url.into());
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
            // Get the cookie manager for the EMBEDDED BROWSER'S
            // RequestContext, not the global default. Each
            // `RequestContext` has its own SQLite cookie store at
            // `<cache_path>/Network/Cookies`; the
            // `cookie_manager_get_global_manager(None)` shortcut
            // returns the DEFAULT context's manager which is
            // backed by `<cache_root>/Default/Network/Cookies` —
            // a different file, not where our browser writes.
            //
            // Falling back to global if we can't reach the
            // per-context manager (e.g., browser not created yet
            // during startup snapshot) preserves the old behavior
            // for that edge case.
            let per_context = browser_slot()
                .lock()
                .ok()
                .and_then(|s| s.as_ref().cloned())
                .and_then(|b| b.host())
                .and_then(|h| h.request_context())
                .and_then(|rc| rc.cookie_manager(None));
            let Some(manager) = per_context.or_else(|| cookie_manager_get_global_manager(None)) else {
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
