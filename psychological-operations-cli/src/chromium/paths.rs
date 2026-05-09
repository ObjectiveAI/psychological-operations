//! Per-OS path helpers for the embedded Chromium subsystem. All
//! rooted at the runtime `Config`'s base dir.

use std::path::PathBuf;

use crate::run::Config as RuntimeConfig;

/// Cache root for the extracted Chromium zip + extension. Each unique
/// embedded payload (content-hashed) gets its own subdirectory.
pub fn chromium_cache_root(cfg: &RuntimeConfig) -> PathBuf {
    cfg.base_dir().join("chromium")
}

/// Per-psyop Chromium profile dir. Persists logins / cookies between runs.
pub fn profile_dir(psyop: &str, cfg: &RuntimeConfig) -> PathBuf {
    cfg.base_dir().join("chromium-profiles").join(psyop)
}

/// Master X-App Chromium profile dir. Distinct from the per-psyop
/// profile tree so a psyop name can never collide.
pub fn x_app_profile_dir(cfg: &RuntimeConfig) -> PathBuf {
    cfg.base_dir().join("chromium-x_app")
}

/// Where the wrapper script that Chromium invokes for native messaging
/// lives. Generated lazily; one-time write per OS user.
pub fn native_host_wrapper(cfg: &RuntimeConfig) -> PathBuf {
    let bin = cfg.base_dir().join("bin");
    if cfg!(windows) {
        bin.join("psychological-operations-native-host.cmd")
    } else {
        bin.join("psychological-operations-native-host.sh")
    }
}

/// Native-messaging-host manifest path for a given Chromium profile.
/// On Windows this isn't actually used at runtime (Chromium reads the
/// manifest path from HKCU registry instead) — kept here only for
/// consistency / debugging.
pub fn native_host_manifest_for_profile(profile: &std::path::Path) -> PathBuf {
    profile.join("NativeMessagingHosts").join(
        format!("{}.json", crate::chromium::bundles::NATIVE_HOST_NAME),
    )
}
