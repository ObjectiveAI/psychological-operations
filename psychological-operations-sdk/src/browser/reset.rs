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
/// * delete its `persona_twids` mapping (the persona → account-twid link),
/// * recursively delete its CEF profile at
///   `<state_dir>/browser/cef-root/<cache_subdir>/`.
///
/// The account's token row (`account_auth`) is deliberately LEFT INTACT —
/// it's keyed by twid, not persona, and the same X account may still be
/// operated by another persona. Resetting a persona just forgets which
/// account it was, so a fresh login can re-establish (or change) it.
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
    db.persona_twid_delete(kind.db_kind(), name)
        .await
        .map_err(|e| format!("delete persona mapping: {e}"))?;
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
/// * delete every account's OAuth token (`account_auth`) — a new X-App
///   orphans every token minted under the old one; the personas'
///   `persona_twids` mappings and their X.com CEF sessions are PRESERVED,
///   so each can simply re-OAuth against the new X-App,
/// * recursively delete the X-App CEF profile at
///   `<state_dir>/browser/cef-root/x-app/`.
///
/// Used by `x-app setup --dangerously-reset` before relaunching, which
/// recaptures fresh snapshots.
pub async fn wipe_x_app(db: &Db, state_dir: &Path) -> Result<(), String> {
    db.x_app_credentials_clear()
        .await
        .map_err(|e| format!("clear x_app credentials: {e}"))?;
    db.account_auth_delete_all()
        .await
        .map_err(|e| format!("delete all account tokens: {e}"))?;
    rm_rf_optional(&state_dir.join("browser").join("cef-root").join("x-app"))
        .map_err(|e| format!("wipe x-app CEF profile: {e}"))?;
    Ok(())
}
