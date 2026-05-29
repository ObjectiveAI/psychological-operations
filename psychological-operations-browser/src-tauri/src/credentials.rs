//! Per-handle X-App credential storage.
//!
//! The content webview's overlay calls
//! [`store_one`] (via the `store_x_app_credential` Tauri command in
//! [`crate::stdio`]) one field at a time as the user copies each
//! credential out of the X developer console. Each field lands in
//! its own `<field>.txt` file under a per-handle directory:
//!
//! ```text
//! <data-dir>/handles/<handle>/
//!   ├── client_id.txt
//!   ├── client_secret.txt
//!   ├── bearer_token.txt
//!   ├── access_token.txt
//!   └── access_token_secret.txt
//! ```
//!
//! `<data-dir>` is the X-App mode's data root
//! ([`crate::webview::mode_data_dir`] with [`Mode::XApp`]).
//! `<handle>` is the user's
//! X handle normalized via [`normalize_handle`] — strip leading
//! `@`, lower-case, validate against X's handle rules (1-15 ASCII
//! alphanumeric / underscore characters).
//!
//! Each file contains exactly the raw credential string — no
//! quoting, no JSON envelope, no trailing newline. Writes go
//! through `<field>.txt.tmp` + atomic rename so a crash mid-write
//! can't leave a truncated value.
//!
//! Partial state is intentional: only the fields the overlay has
//! sent yet are on disk. The CLI later reads what's there and
//! treats missing files as "not set yet."

use std::fs;
use std::path::PathBuf;

use psychological_operations_browser_sdk::credentials::XAppCredentialField;
use psychological_operations_browser_sdk::mode::{self, Mode};
use tauri::{AppHandle, Wry};

use crate::webview;

/// Write a single credential field for a given X handle. Returns
/// the resolved path on success. Errors propagate as `String` so
/// they can flow straight back through the Tauri command boundary.
///
/// Credentials only make sense in X-App mode (the X developer
/// console is where they come from). Calls from any other mode
/// return an error rather than writing to the wrong directory.
pub fn store_one(
    app: &AppHandle<Wry>,
    handle: &str,
    field: XAppCredentialField,
    value: &str,
) -> Result<PathBuf, String> {
    let handle = normalize_handle(handle)?;
    if !matches!(mode::get(), Some(Mode::XApp)) {
        return Err("credentials::store_one called outside X-App mode".into());
    }
    let dir = webview::mode_data_dir(app, &Mode::XApp)
        .join("handles")
        .join(&handle);
    fs::create_dir_all(&dir).map_err(|e| format!("create handle dir: {e}"))?;

    let final_path = dir.join(field.file_name());
    let tmp_path = dir.join(format!("{}.tmp", field.file_name()));
    fs::write(&tmp_path, value.as_bytes())
        .map_err(|e| format!("write tmp credential: {e}"))?;
    fs::rename(&tmp_path, &final_path)
        .map_err(|e| format!("rename tmp credential: {e}"))?;
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
/// `true` iff all three X-App OAuth credentials we care about
/// (consumer key, secret key, bearer token) are present on disk
/// under `handles/<normalize_handle(user_id)>/`. Used by
/// [`crate::state::derive`] to decide whether the panel still
/// needs to nudge the user through the create-app flow.
///
/// Cheap: three `Path::exists` checks. Returns `false` on any
/// disk error or invalid handle — anything we can't confidently
/// read is treated as "creds missing", which is the safe default
/// for the UX (worst case: we ask the user to redo the flow).
pub fn all_three_present(app: &AppHandle<Wry>, user_id: &str) -> bool {
    let Ok(handle) = normalize_handle(user_id) else { return false };
    let dir = webview::mode_data_dir(app, &Mode::XApp)
        .join("handles")
        .join(&handle);
    [
        XAppCredentialField::ConsumerKey,
        XAppCredentialField::SecretKey,
        XAppCredentialField::BearerToken,
    ]
    .iter()
    .all(|f| dir.join(f.file_name()).exists())
}

/// `true` iff both per-user OAuth 1.0a access-token fields
/// (`access_token`, `access_token_secret`) are on disk under
/// `handles/<normalize_handle(user_id)>/`. The post-create dialog
/// doesn't surface these — they're generated separately from
/// the app's Keys & Tokens page — so this is tracked as a
/// distinct fact from the first-three triple. Same defensive
/// "any error → false" posture as [`all_three_present`].
pub fn access_tokens_present(app: &AppHandle<Wry>, user_id: &str) -> bool {
    let Ok(handle) = normalize_handle(user_id) else { return false };
    let dir = webview::mode_data_dir(app, &Mode::XApp)
        .join("handles")
        .join(&handle);
    [
        XAppCredentialField::AccessToken,
        XAppCredentialField::AccessTokenSecret,
    ]
    .iter()
    .all(|f| dir.join(f.file_name()).exists())
}

fn normalize_handle(raw: &str) -> Result<String, String> {
    let trimmed = raw
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase();
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
