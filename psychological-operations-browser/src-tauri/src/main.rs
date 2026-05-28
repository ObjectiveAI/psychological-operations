// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // CEF helper-subprocess short-circuit. On Windows/Linux, Chromium
    // re-invokes this same binary for renderer/GPU/utility processes
    // with `--type=<kind>` on the command line. Detect it BEFORE
    // clap runs (so the helper's switches don't trip clap) and route
    // to CEF's helper entry; that call never returns.
    //
    // On macOS helpers are separate .app bundles, so
    // `is_helper_process` always returns false there.
    #[cfg(not(target_os = "macos"))]
    if psychological_operations_browser_lib::cef::is_helper_process() {
        psychological_operations_browser_lib::cef::run_helper_and_exit();
    }

    psychological_operations_browser_lib::run()
}
