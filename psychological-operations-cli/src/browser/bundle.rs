//! Compile-time-embedded browser bundle.
//!
//! Paths come from `build.rs`, which validates that
//! `psychological-operations-browser/embed/<target>/<profile>/`
//! contains both `browser-bundle.zip` and `browser-entry.txt`,
//! then emits `cargo:rustc-env` lines.
//!
//! The bundle holds the CEF browser exe + its full runtime
//! (libcef.dll, *.pak, locales/, etc.). The entry text gives the
//! bundle-relative path to the launchable exe (e.g.
//! `psychological-operations-browser.exe`).

pub const BROWSER_BUNDLE: &[u8] = include_bytes!(env!("PSYOPS_BROWSER_BUNDLE_PATH"));

const BROWSER_ENTRY_RAW: &str = include_str!(env!("PSYOPS_BROWSER_ENTRY_PATH"));

pub fn browser_entry() -> &'static str {
    BROWSER_ENTRY_RAW.trim()
}
