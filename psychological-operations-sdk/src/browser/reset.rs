//! Best-effort wipe of a persona's / the X-App's state. OAuth tokens
//! and captured HTML now live in postgres, so a wipe spans both the db
//! (token / snapshot rows) and the filesystem (the per-context CEF
//! profile under `cef-root/`, which is still on disk). Used by the
//! CLI's `--dangerously-reset` login + `x-app setup` paths.
//!
//! `NotFound` on the CEF dir is swallowed (the persona never opened a
//! browser here); other I/O errors bubble up so the caller can refuse
//! to proceed with half-wiped state.

use std::path::Path;

use psychological_operations_db::Db;

use super::auth_json::PersonaKind;

/// Wipe one persona's state:
///
/// * delete its `auth_tokens` rows (every persona_twid × x_app_twid),
/// * recursively delete its CEF profile at
///   `<state_dir>/browser/cef-root/<cache_subdir>/`.
///
/// The CEF subdir comes from [`Mode::cache_subdir`] (via
/// [`PersonaKind::to_mode`]) so it matches exactly what the browser
/// wrote. Each persona has its own flat profile dir (a direct child of
/// `cef-root`), so removing it can never touch another persona.
pub async fn wipe_persona(
    db: &Db,
    state_dir: &Path,
    kind: PersonaKind,
    name: &str,
) -> Result<(), String> {
    db.auth_delete_persona(kind.db_kind(), name)
        .await
        .map_err(|e| format!("delete persona tokens: {e}"))?;
    let cef_subdir = kind.to_mode(name).cache_subdir();
    let profile = state_dir.join("browser").join("cef-root").join(&cef_subdir);
    rm_rf_optional(&profile).map_err(|e| format!("wipe persona CEF profile: {e}"))?;
    Ok(())
}

fn rm_rf_optional(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Wipe X-App state:
///
/// * clear ALL captured credential HTML snapshots (`x_app_html`) — both
///   the `post_create_dialog` and `oauth_popup` rows, for every handle;
///   these snapshots are the source of truth for the App's credentials,
/// * recursively delete the X-App CEF profile at
///   `<state_dir>/browser/cef-root/x-app/`.
///
/// Used by `x-app setup --dangerously-reset` before relaunching, which
/// recaptures fresh snapshots.
pub async fn wipe_x_app(db: &Db, state_dir: &Path) -> Result<(), String> {
    db.x_app_html_clear()
        .await
        .map_err(|e| format!("clear x_app html: {e}"))?;
    rm_rf_optional(&state_dir.join("browser").join("cef-root").join("x-app"))
        .map_err(|e| format!("wipe x-app CEF profile: {e}"))?;
    Ok(())
}

/// Delete every persona's OAuth tokens. CEF cookies for those personas
/// are intentionally PRESERVED — wiping the X-App orphans each persona's
/// prior tokens (minted under the old `x_app_twid`), but their X.com
/// sessions are still valid and can be re-OAuthed against the new X-App
/// without re-signing-in. Used by `x-app setup --dangerously-reset`.
pub async fn wipe_all_persona_auth(db: &Db) -> Result<(), String> {
    db.auth_delete_all()
        .await
        .map_err(|e| format!("delete all persona tokens: {e}"))
}
