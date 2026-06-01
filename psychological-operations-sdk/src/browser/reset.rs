//! Best-effort wipe of every on-disk artifact for one persona —
//! both the per-persona OAuth tokens dir AND the per-persona CEF
//! profile (cookies). Used by the CLI's
//! `psychological-operations psyops login --dangerously-reset` /
//! `agents login --dangerously-reset` path.
//!
//! `NotFound` on either side is swallowed (the persona never
//! signed in here in the first place); any other I/O error
//! bubbles up so the caller can refuse to proceed with a
//! half-wiped state.

use std::path::Path;

use super::auth_json::PersonaKind;

/// Recursively delete both subdirs that hold a persona's state:
///
/// * `<config>/plugins/psychological-operations/browser/<kind>/<name>/`
///   — the OAuth tokens / `auth.json` tree.
/// * `<config>/plugins/psychological-operations/browser/cef-root/<kind>-<name>/`
///   — the CEF profile, including the cookies SQLite store. The
///   `<kind>-<name>` naming mirrors `cookies::cache_subdir_for`
///   for `Mode::{PsyopRead, PsyopAuthorize, AgentAuthorize}` —
///   the two ends MUST stay in sync.
pub fn wipe_persona(
    config_base_dir: &Path,
    kind: PersonaKind,
    name: &str,
) -> std::io::Result<()> {
    let browser_root = config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser");
    let kind_seg = match kind {
        PersonaKind::Psyop => "psyop",
        PersonaKind::Agent => "agent",
    };
    let cef_subdir = format!("{kind_seg}-{name}");
    rm_rf_optional(&browser_root.join(kind_seg).join(name))?;
    rm_rf_optional(&browser_root.join("cef-root").join(&cef_subdir))?;
    Ok(())
}

fn rm_rf_optional(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Recursively delete both X-App subdirs:
///
/// * `<config>/plugins/psychological-operations/browser/x-app/`
///   — the HTML snapshots dir.
/// * `<config>/plugins/psychological-operations/browser/cef-root/x-app/`
///   — the CEF profile (cookies, IndexedDB, etc.).
///
/// `NotFound` on either side is swallowed (clean state already).
/// Used by `x_app setup --dangerously-reset` before relaunching
/// the browser into a fresh state.
pub fn wipe_x_app(config_base_dir: &Path) -> std::io::Result<()> {
    let browser_root = config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser");
    rm_rf_optional(&browser_root.join("x-app"))?;
    rm_rf_optional(&browser_root.join("cef-root").join("x-app"))?;
    Ok(())
}

/// Recursively delete every named persona dir under
/// `<config>/.../browser/psyop/` and
/// `<config>/.../browser/agent/`. CEF cookies for those personas
/// (under `cef-root/<kind>-<name>/`) are intentionally
/// **PRESERVED** — wiping the X-App means each persona's prior
/// `auth.json` (minted under the old `x_app_twid`) is orphaned,
/// but their X.com sessions are still valid and can be
/// re-OAuthed against the new X-App without re-signing-in.
///
/// `NotFound` on the parent dirs (psyop/, agent/) is fine — those
/// directories may not exist yet on a fresh install.
pub fn wipe_all_persona_auth_dirs(config_base_dir: &Path) -> std::io::Result<()> {
    let browser_root = config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser");
    for kind_seg in ["psyop", "agent"] {
        let kind_dir = browser_root.join(kind_seg);
        let entries = match std::fs::read_dir(&kind_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                rm_rf_optional(&entry.path())?;
            }
        }
    }
    Ok(())
}
