//! In-memory X-App credential snapshot capture.
//!
//! Holds the two X developer-console credential HTML surfaces — the
//! post-create dialog and the OAuth 2.0 settings popup — for the single X
//! account being set up. The browser no longer persists them: `x-app setup`
//! emits both (via [`captured`]) in one `XAppSetupSucceeded` item at the end
//! and the CLI writes them to the DB. Parsing lives in
//! [`psychological_operations_sdk::browser::x_app_credentials`].

use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct Capture {
    handle: Option<String>,
    post_create_dialog: Option<String>,
    oauth_popup: Option<String>,
}

fn slot() -> &'static Mutex<Capture> {
    static SLOT: OnceLock<Mutex<Capture>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Capture::default()))
}

/// Stash the post-create dialog HTML snapshot for `handle` (the signed-in
/// twid).
pub fn save_post_create_dialog(handle: &str, html: &str) -> Result<(), String> {
    let h = normalize_handle(handle)?;
    let mut c = slot().lock().expect("capture slot poisoned");
    c.handle = Some(h);
    c.post_create_dialog = Some(html.to_string());
    Ok(())
}

/// Stash the OAuth 2.0 settings popup HTML snapshot for `handle`.
pub fn save_oauth_popup(handle: &str, html: &str) -> Result<(), String> {
    let h = normalize_handle(handle)?;
    let mut c = slot().lock().expect("capture slot poisoned");
    c.handle = Some(h);
    c.oauth_popup = Some(html.to_string());
    Ok(())
}

/// Whether a post-create snapshot has been captured for `handle`.
pub fn post_create_present(handle: &str) -> bool {
    let Ok(h) = normalize_handle(handle) else {
        return false;
    };
    let c = slot().lock().expect("capture slot poisoned");
    c.handle.as_deref() == Some(h.as_str()) && c.post_create_dialog.is_some()
}

/// Whether an OAuth popup snapshot has been captured for `handle`.
pub fn oauth_popup_present(handle: &str) -> bool {
    let Ok(h) = normalize_handle(handle) else {
        return false;
    };
    let c = slot().lock().expect("capture slot poisoned");
    c.handle.as_deref() == Some(h.as_str()) && c.oauth_popup.is_some()
}

/// The full capture (handle + both HTML blobs) once both are present — what
/// `x-app setup` emits for the CLI to persist. `None` until both captured.
pub fn captured() -> Option<(String, String, String)> {
    let c = slot().lock().expect("capture slot poisoned");
    Some((
        c.handle.clone()?,
        c.post_create_dialog.clone()?,
        c.oauth_popup.clone()?,
    ))
}

/// Normalize an X handle (or numeric user-id) for use as the capture key:
/// trim, strip leading `@`, lower-case, and validate length 1-64 ASCII
/// alphanumeric / underscore (64 so numeric twids fit).
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
