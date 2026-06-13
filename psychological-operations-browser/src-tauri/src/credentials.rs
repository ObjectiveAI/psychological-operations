//! Per-handle X-App credential snapshot storage.
//!
//! Each of the two X developer-console credential surfaces — the
//! post-create dialog and the OAuth 2.0 settings popup — is captured as
//! a raw HTML snapshot in the db crate's `x_app_html` table, keyed by
//! `(handle, kind)`. `<handle>` is the user's X handle / numeric twid
//! normalized via [`normalize_handle`]. Parsing lives in
//! [`psychological_operations_sdk::browser::x_app_credentials`]; callers
//! reach the values through that module's `from_db` constructors.

use psychological_operations_db::Db;
use psychological_operations_sdk::browser::x_app_credentials::{
    OAUTH_POPUP_KIND, POST_CREATE_DIALOG_KIND,
};
use tauri::{AppHandle, Manager, Wry};

/// Store the post-create dialog HTML snapshot for `handle`.
pub async fn save_post_create_dialog(
    app: &AppHandle<Wry>,
    handle: &str,
    html: &str,
) -> Result<(), String> {
    save(app, handle, POST_CREATE_DIALOG_KIND, html).await
}

/// Store the OAuth 2.0 settings popup HTML snapshot for `handle`.
pub async fn save_oauth_popup(
    app: &AppHandle<Wry>,
    handle: &str,
    html: &str,
) -> Result<(), String> {
    save(app, handle, OAUTH_POPUP_KIND, html).await
}

/// Presence check for the post-create dialog snapshot under `handle`.
/// `false` on any error or invalid handle — matches the defensive
/// posture the old per-field presence checks used.
pub async fn post_create_present(app: &AppHandle<Wry>, handle: &str) -> bool {
    present(app, handle, POST_CREATE_DIALOG_KIND).await
}

/// Presence check for the OAuth popup snapshot under `handle`.
pub async fn oauth_popup_present(app: &AppHandle<Wry>, handle: &str) -> bool {
    present(app, handle, OAUTH_POPUP_KIND).await
}

async fn save(
    app: &AppHandle<Wry>,
    handle: &str,
    kind: &str,
    html: &str,
) -> Result<(), String> {
    let handle_norm = normalize_handle(handle)?;
    let db = app.state::<Db>();
    db.x_app_html_set(&handle_norm, kind, html)
        .await
        .map_err(|e| format!("store {kind} snapshot: {e}"))
}

async fn present(app: &AppHandle<Wry>, handle: &str, kind: &str) -> bool {
    let Ok(handle_norm) = normalize_handle(handle) else {
        return false;
    };
    let db = app.state::<Db>();
    db.x_app_html_present(&handle_norm, kind).await.unwrap_or(false)
}

/// Normalize an X handle (or numeric user-id) for use as a snapshot
/// key:
///
///   - trim surrounding whitespace
///   - strip leading `@` (X displays handles with one)
///   - lower-case (handles are case-insensitive)
///   - validate length 1-64 and ASCII alphanumeric / underscore.
///     Length cap is 64 (not 15) so X numeric user-ids parsed from the
///     `twid` cookie also fit — those range from ~7 digits (early-era
///     accounts) to ~19 digits (modern Snowflake IDs).
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
