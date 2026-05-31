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
