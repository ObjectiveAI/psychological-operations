//! CEF helper subprocess entrypoint — macOS only.
//!
//! On macOS, CEF spawns each subprocess (renderer, GPU, utility,
//! plugin) as a SEPARATE executable inside its own `.app` bundle
//! under `<MainApp>.app/Contents/Frameworks/`. Chromium can't
//! re-invoke the main binary with `--type=...` (the way it does on
//! Windows/Linux) because macOS app-bundle isolation requires
//! each subprocess to have its own bundle.
//!
//! `bundle-cef-app` (from the `cef` crate's `build-util` feature)
//! produces the helper `.app` structure, copies this binary into
//! its `Contents/MacOS/`, and points CEF at it via
//! `Settings.browser_subprocess_path` (set in
//! [`psychological_operations_browser_lib::cef::initialize`]).
//!
//! On Windows / Linux this binary still builds (cargo's `[[bin]]`
//! doesn't gate per-platform) but is never invoked — the main
//! binary is its own helper via `--type=...` short-circuit in
//! [`crate::main`].

#![cfg_attr(
    all(not(debug_assertions), not(target_os = "macos")),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "macos")]
fn main() {
    use cef::*;

    // Load CEF framework from the parent app's
    // `Contents/Frameworks/`. The `true` flag tells LibraryLoader
    // we're a helper (one level deeper than the main binary), so it
    // walks up to find the framework instead of expecting it as a
    // sibling.
    let loader = library_loader::LibraryLoader::new(
        &std::env::current_exe().expect("current_exe"),
        true,
    );
    assert!(loader.load(), "failed to load CEF framework in helper");

    let args = args::Args::new();
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    // execute_process runs the helper's message loop to completion
    // and returns the helper's exit code; we just propagate.
    let _ret = execute_process(
        Some(args.as_main_args()),
        None::<&mut App>,
        std::ptr::null_mut(),
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    // Never invoked on Windows/Linux (the main binary handles
    // helpers itself via the `--type=...` short-circuit). Exit
    // cleanly so accidental invocations don't loop.
    std::process::exit(0);
}
