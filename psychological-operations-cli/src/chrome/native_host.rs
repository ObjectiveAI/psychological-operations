//! Native-messaging-host registration so the embedded Chromium can
//! invoke `psychological-operations native-host` over stdio when the
//! installed extension calls `chrome.runtime.connectNative(...)`.
//!
//! Per Chromium's docs, the lookup paths are:
//!
//!   Windows: HKCU\SOFTWARE\Google\Chrome\NativeMessagingHosts\<name>
//!            (default value = absolute path to the manifest JSON)
//!
//!   Linux:   <user-data-dir>/NativeMessagingHosts/<name>.json
//!            ~/.config/google-chrome/NativeMessagingHosts/<name>.json (fallback)
//!
//!   macOS:   <user-data-dir>/NativeMessagingHosts/<name>.json
//!            ~/Library/Application Support/Google/Chrome/NativeMessagingHosts/<name>.json
//!            (similar fallback)
//!
//! On Windows we register once in HKCU. Per-profile reuse falls out
//! of the registry being per-user. On Linux/Mac we drop the manifest
//! into the profile's `NativeMessagingHosts/` dir.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::bundles::{EXTENSION_ID, NATIVE_HOST_NAME, extension_id};
use super::paths::{native_host_manifest_for_profile, native_host_wrapper};
use crate::error::Error;

/// Write the manifest into the profile's NativeMessagingHosts dir
/// (Linux/macOS) or the per-user HKCU registry (Windows). Idempotent
/// — overwrites if already present.
pub fn install(profile: &Path, cfg: &crate::run::Config) -> Result<(), Error> {
    let wrapper = ensure_wrapper(cfg)?;
    let manifest_path_for_registry: PathBuf;

    #[cfg(not(windows))]
    {
        let manifest_path = native_host_manifest_for_profile(profile);
        if let Some(parent) = manifest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let manifest = json!({
            "name": NATIVE_HOST_NAME,
            "description": "Psychological Operations native host",
            "path": wrapper,
            "type": "stdio",
            "allowed_origins": [format!("chrome-extension://{}/", extension_id())],
        });
        fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
        manifest_path_for_registry = manifest_path;
    }

    #[cfg(windows)]
    {
        // Drop a JSON next to the wrapper too — diagnostic + fallback
        // for users who manually configured Chromium with an external
        // user-data-dir not driven by us.
        let manifest_path = native_host_manifest_for_profile(profile);
        if let Some(parent) = manifest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let manifest = json!({
            "name": NATIVE_HOST_NAME,
            "description": "Psychological Operations native host",
            "path": wrapper.to_string_lossy().replace('\\', "\\\\"),
            "type": "stdio",
            "allowed_origins": [format!("chrome-extension://{}/", extension_id())],
        });
        fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
        register_windows_native_host(&manifest_path)?;
        manifest_path_for_registry = manifest_path;
    }

    let _ = manifest_path_for_registry;
    let _ = EXTENSION_ID;
    Ok(())
}

/// Lazily generate the tiny wrapper script that Chromium invokes
/// (Chromium's manifest `path` field doesn't accept args, so we need
/// a separate executable that just exec's our binary with the
/// `native-host` subcommand).
fn ensure_wrapper(cfg: &crate::run::Config) -> Result<PathBuf, Error> {
    let path = native_host_wrapper(cfg);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let exe = std::env::current_exe()?;
    #[cfg(windows)]
    let body = format!(
        "@echo off\r\n\"{}\" native-host %*\r\n",
        exe.display(),
    );
    #[cfg(not(windows))]
    let body = format!(
        "#!/bin/sh\nexec \"{}\" native-host \"$@\"\n",
        exe.display(),
    );
    fs::write(&path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok(path)
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
