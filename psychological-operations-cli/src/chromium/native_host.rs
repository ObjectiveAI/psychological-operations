//! Native-messaging-host registration so the embedded Chromium can
//! invoke `psychological-operations native-host` over stdio when the
//! installed extension calls `chrome.runtime.connectNative(...)`.
//!
//! Per Chromium's docs, the lookup paths are:
//!
//!   Windows: HKCU\SOFTWARE\Chromium\NativeMessagingHosts\<name>
//!            (default value = absolute path to the manifest JSON;
//!            we also write to HKCU\SOFTWARE\Google\Chrome\... so a
//!            user-installed Google Chrome with our extension side-
//!            loaded can find the host too.)
//!
//!   Linux:   <user-data-dir>/NativeMessagingHosts/<name>.json
//!            ~/.config/chromium/NativeMessagingHosts/<name>.json (fallback)
//!
//!   macOS:   <user-data-dir>/NativeMessagingHosts/<name>.json
//!            ~/Library/Application Support/Chromium/NativeMessagingHosts/<name>.json
//!            (similar fallback)
//!
//! On Windows we register once in HKCU. Per-profile reuse falls out
//! of the registry being per-user. On Linux/Mac we drop the manifest
//! into the profile's `NativeMessagingHosts/` dir.

use std::fs;
use std::path::Path;

use serde_json::json;

use super::bundles::{
    AUTH_EXTENSION_ID, NATIVE_HOST_NAME, READ_EXTENSION_ID,
    auth_extension_id, read_extension_id,
};
use super::paths::native_host_manifest_for_profile;
use crate::error::Error;

/// Write the manifest into the profile's NativeMessagingHosts dir
/// (Linux/macOS) or the per-user HKCU registry (Windows). Idempotent
/// — overwrites if already present.
pub fn install(profile: &Path, _cfg: &crate::run::Config) -> Result<(), Error> {
    // Point the manifest at the main psychological-operations binary
    // directly. main.rs detects the Chromium-NM-host invocation
    // signature (`--parent-window=` arg etc.) and routes to
    // native-host mode automatically. This avoids a .cmd / .sh
    // wrapper, which on Windows mangles binary stdin under cmd.exe
    // and breaks the NM length-prefix protocol.
    let exe = std::env::current_exe()?;

    let manifest_path = native_host_manifest_for_profile(profile);
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let manifest = json!({
        "name": NATIVE_HOST_NAME,
        "description": "Psychological Operations native host",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [
            format!("chrome-extension://{}/", read_extension_id()),
            format!("chrome-extension://{}/", auth_extension_id()),
        ],
    });
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    #[cfg(windows)]
    register_windows_native_host(&manifest_path)?;

    let _ = READ_EXTENSION_ID;
    let _ = AUTH_EXTENSION_ID;
    Ok(())
}


#[cfg(windows)]
fn register_windows_native_host(manifest_path: &Path) -> Result<(), Error> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // Register under both Chrome and Chromium roots so it works
    // regardless of which Chromium-derivative is in play.
    for vendor_subkey in [
        format!("SOFTWARE\\Google\\Chrome\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
        format!("SOFTWARE\\Chromium\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
    ] {
        let (key, _) = hkcu
            .create_subkey(&vendor_subkey)
            .map_err(|e| Error::Other(format!("registry create_subkey: {e}")))?;
        key.set_value("", &manifest_path.to_string_lossy().to_string())
            .map_err(|e| Error::Other(format!("registry set_value: {e}")))?;
    }
    Ok(())
}
