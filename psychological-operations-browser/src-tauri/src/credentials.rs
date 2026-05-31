//! Per-handle X-App credential snapshot storage.
//!
//! Each of the two X developer-console credential surfaces — the
//! post-create dialog and the OAuth 2.0 settings popup — is
//! captured as a raw HTML snapshot at:
//!
//! ```text
//! <data-dir>/handles/<handle>/
//!   ├── post_create_dialog.html   (covers consumer_key, secret_key, bearer_token)
//!   └── oauth_popup.html          (covers client_id, client_secret)
//! ```
//!
//! `<data-dir>` is the X-App mode's data root
//! ([`crate::webview::mode_data_dir`] with [`Mode::XApp`]).
//! `<handle>` is the user's X handle normalized via
//! [`normalize_handle`] (strip leading `@`, lower-case, validate).
//!
//! Writes go through `<file>.tmp` + atomic rename so a crash
//! mid-write can't leave a truncated file on disk. Parsing lives
//! in [`psychological_operations_sdk::browser::x_app_credentials`];
//! callers reach the values through that module.

use std::path::PathBuf;

use psychological_operations_sdk::browser::mode::Mode;
use tauri::{AppHandle, Wry};
use tokio::fs;

use crate::webview;

pub const POST_CREATE_DIALOG_FILE: &str = "post_create_dialog.html";
pub const OAUTH_POPUP_FILE: &str = "oauth_popup.html";

/// Write the post-create dialog HTML snapshot for `handle`. Returns
/// the resolved final path on success.
pub async fn save_post_create_dialog(
    app: &AppHandle<Wry>,
    handle: &str,
    html: &str,
) -> Result<PathBuf, String> {
    save(app, handle, POST_CREATE_DIALOG_FILE, html).await
}

/// Write the OAuth 2.0 settings popup HTML snapshot for `handle`.
/// Returns the resolved final path on success.
pub async fn save_oauth_popup(
    app: &AppHandle<Wry>,
    handle: &str,
    html: &str,
) -> Result<PathBuf, String> {
    save(app, handle, OAUTH_POPUP_FILE, html).await
}

/// Resolve the per-handle snapshot path without writing. `None`
/// only when `handle` fails to normalize (invalid X handle shape).
/// Used by [`crate::state`] for presence-check derivation and by
/// readers that just want the path.
pub fn snapshot_path(
    app: &AppHandle<Wry>,
    handle: &str,
    file_name: &str,
) -> Option<PathBuf> {
    let handle_norm = normalize_handle(handle).ok()?;
    Some(
        webview::mode_data_dir(app, &Mode::XApp)
            .join("handles")
            .join(handle_norm)
            .join(file_name),
    )
}

/// Presence check for the post-create dialog snapshot under
/// `handle`. `false` on any error or invalid handle — matches the
/// defensive posture the old per-field presence checks used.
pub async fn post_create_present(app: &AppHandle<Wry>, handle: &str) -> bool {
    match snapshot_path(app, handle, POST_CREATE_DIALOG_FILE) {
        Some(p) => fs::try_exists(&p).await.unwrap_or(false),
        None => false,
    }
}

/// Presence check for the OAuth popup snapshot under `handle`.
/// Same defensive posture as [`post_create_present`].
pub async fn oauth_popup_present(app: &AppHandle<Wry>, handle: &str) -> bool {
    match snapshot_path(app, handle, OAUTH_POPUP_FILE) {
        Some(p) => fs::try_exists(&p).await.unwrap_or(false),
        None => false,
    }
}

async fn save(
    app: &AppHandle<Wry>,
    handle: &str,
    file_name: &str,
    html: &str,
) -> Result<PathBuf, String> {
    let handle_norm = normalize_handle(handle)?;
    let dir = webview::mode_data_dir(app, &Mode::XApp)
        .join("handles")
        .join(&handle_norm);
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create handle dir: {e}"))?;

    let final_path = dir.join(file_name);
    let tmp_path = dir.join(format!("{file_name}.tmp"));
    fs::write(&tmp_path, html.as_bytes())
        .await
        .map_err(|e| format!("write tmp snapshot: {e}"))?;
    fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| format!("rename tmp snapshot: {e}"))?;
    Ok(final_path)
}

/// Normalize an X handle (or numeric user-id) for use as a
/// directory name:
///
///   - trim surrounding whitespace
///   - strip leading `@` (X displays handles with one)
///   - lower-case (handles are case-insensitive)
///   - validate length 1-64 and ASCII alphanumeric / underscore.
///     Length cap is 64 (not 15) so X numeric user-ids parsed
///     from the `twid` cookie also fit — those range from
///     ~7 digits (early-era accounts) to ~19 digits (modern
///     Snowflake IDs).
fn normalize_handle(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_start_matches('@').to_ascii_lowercase();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(format!("invalid X handle: {raw:?}"));
    }
    for c in trimmed.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("invalid character in X handle: {raw:?}"));
        }
    }
    Ok(trimmed)
}

