//! Compile-time-embedded Chromium zip + extension assets.
//!
//! Paths come from `build.rs`, which calls
//! `psychological-operations-chromium/validate.sh` to confirm the
//! sister-bundle is present and fresh, then emits `cargo:rustc-env`
//! lines pointing at each artifact in `embed/<target>/<profile>/`.

pub const CHROMIUM_BUNDLE: &[u8] = include_bytes!(env!("PSYOPS_CHROMIUM_BUNDLE_PATH"));
pub const EXTENSION_TAR: &[u8] = include_bytes!(env!("PSYOPS_EXTENSION_TAR_PATH"));

/// 32-char extension ID derived from the SPKI public key in
/// `psychological-operations-chromium/extension-key.pem`. Stable across
/// every build because the key is committed.
pub const EXTENSION_ID: &str = include_str!(env!("PSYOPS_EXTENSION_ID_PATH"));

/// Relative path inside the extracted Chromium zip to the launchable
/// binary (e.g. `chrome-win/chrome.exe`).
pub const LAUNCH_ENTRY: &str = include_str!(env!("PSYOPS_CHROMIUM_LAUNCH_ENTRY_PATH"));

/// Trim trailing whitespace once at lookup time — the build-time
/// `*.txt` files end in a newline.
pub fn extension_id() -> &'static str {
    EXTENSION_ID.trim()
}

pub fn launch_entry() -> &'static str {
    LAUNCH_ENTRY.trim()
}

/// Reserved native-messaging host name used by the extension and the
/// `psychological-operations native-host` subcommand. Same string is
/// hard-coded in `psychological-operations-chromium-extension/background.js`.
pub const NATIVE_HOST_NAME: &str = "com.objectiveai.psychological_operations";
