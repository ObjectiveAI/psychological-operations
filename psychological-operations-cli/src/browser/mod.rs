//! psychological-operations-browser subsystem.
//!
//! The CEF browser exe + its full CEF runtime (libcef.dll, *.pak,
//! locales/, …) ship alongside the CLI in the plugin's binaries dir
//! (`OBJECTIVEAI_BIN_DIR` — the release zip's contents, which the host
//! extracts into `<plugin>/cli/`). On `psyops browse` / `psyops login` /
//! `agents login` / `x-app setup` the browser exe is spawned straight
//! from there with the right mode flag + `--state-dir <state_dir>` — no
//! compile-time embedding, no runtime extraction, no temp dir.

pub mod launch;
pub mod stream;

use std::path::PathBuf;

/// Browser exe path WITHIN [`crate::run::Config::bin_dir`] — matches the
/// per-platform `browser-entry.txt` the bundle build writes. The CEF
/// runtime sits beside it in the same dir, so the exe resolves its
/// runtime relative to itself. macOS ships a `.app` bundle, so the exe
/// is nested inside it.
pub const BROWSER_BINARY: &str = if cfg!(target_os = "windows") {
    "psychological-operations-browser.exe"
} else if cfg!(target_os = "macos") {
    "psychological-operations-browser.app/Contents/MacOS/psychological-operations-browser"
} else {
    "psychological-operations-browser"
};

/// Absolute path to the browser exe: `<bin_dir>/<BROWSER_BINARY>`.
pub fn browser_binary(cfg: &crate::run::Config) -> PathBuf {
    cfg.bin_dir().join(BROWSER_BINARY)
}
