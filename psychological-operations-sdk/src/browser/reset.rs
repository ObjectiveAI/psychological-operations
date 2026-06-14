//! Best-effort wipe of a persona's / the X-App's state. OAuth tokens
//! and captured HTML now live in postgres, so a wipe spans both the db
//! (token / snapshot rows) and the filesystem (the per-context CEF
//! profile under `cef-root/`, which is still on disk). Used by the
//! CLI's `--dangerously-reset` login + `x_app setup` paths.
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
/// * delete its **own** CEF profile artifacts at
///   `<state_dir>/browser/cef-root/<cache_subdir>/`, while **sparing**
///   the [`SUBAGENT_DIR`] (`agents/`) child — that subtree holds the CEF
///   profiles of descendant personas, which must survive the parent's
///   reset.
///
/// The CEF subdir comes from [`Mode::cache_subdir`] (via
/// [`PersonaKind::to_mode`]) so it matches exactly what the browser
/// wrote — including the interspersed `agents/` directories of a
/// slash-bearing AIH.
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
    wipe_profile_keep_subagents(&profile)
        .map_err(|e| format!("wipe persona CEF profile: {e}"))?;
    Ok(())
}

/// Delete every entry directly inside `profile` EXCEPT a child directory
/// named [`SUBAGENT_DIR`]. This clears the persona's own Chromium
/// profile (cookies/cache/storage) but leaves the `agents/` subtree —
/// the descendant personas' profiles — untouched. `agents` is never a
/// Chromium artifact name, so nothing of the persona's own is spared.
/// A missing `profile` dir is a no-op.
fn wipe_profile_keep_subagents(profile: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(profile) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str() == Some(super::mode::SUBAGENT_DIR) {
            continue; // preserve descendant personas
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
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
/// * clear all captured credential HTML snapshots (`x_app_html`),
/// * recursively delete the X-App CEF profile at
///   `<state_dir>/browser/cef-root/x-app/`.
///
/// The X-App credential singleton (`x_app`) is intentionally left
/// alone — `x_app setup` recaptures the HTML and re-derives it. Used by
/// `x_app setup --dangerously-reset` before relaunching.
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
/// without re-signing-in. Used by `x_app setup --dangerously-reset`.
pub async fn wipe_all_persona_auth(db: &Db) -> Result<(), String> {
    db.auth_delete_all()
        .await
        .map_err(|e| format!("delete all persona tokens: {e}"))
}
