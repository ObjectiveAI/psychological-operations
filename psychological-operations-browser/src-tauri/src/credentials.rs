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
//! ([`crate::webview::x_app_data_dir`]). `<handle>` is the user's
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
use tauri::{AppHandle, Runtime};

use crate::webview;

/// Write a single credential field for a given X handle. Returns
/// the resolved path on success. Errors propagate as `String` so
/// they can flow straight back through the Tauri command boundary.
pub fn store_one<R: Runtime>(
    app: &AppHandle<R>,
    handle: &str,
    field: XAppCredentialField,
    value: &str,
) -> Result<PathBuf, String> {
    let handle = normalize_handle(handle)?;
    let dir = webview::x_app_data_dir(app)
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

/// Normalize an X handle for use as a directory name:
///
///   - trim surrounding whitespace
///   - strip leading `@` (X displays handles with one)
///   - lower-case (handles are case-insensitive)
///   - validate length 1-15 and ASCII alphanumeric / underscore
///     (X's documented handle rules)
fn normalize_handle(raw: &str) -> Result<String, String> {
    let trimmed = raw
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase();
    if trimmed.is_empty() || trimmed.len() > 15 {
        return Err(format!("invalid X handle: {raw:?}"));
    }
    for c in trimmed.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("invalid character in X handle: {raw:?}"));
        }
    }
    Ok(trimmed)
}
