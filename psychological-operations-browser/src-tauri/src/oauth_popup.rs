//! Parses X's OAuth 2.0 popup that fires after Save Changes
//! on the auth-settings page. Two credentials are surfaced
//! once and only once: Client ID and Client Secret.
//!
//! Structurally a twin of [`crate::post_create_dialog`]:
//! snapshot HTML to disk *before* parsing (so a selector
//! miss still leaves a recoverable fixture at
//! `<x-app-data-dir>/recordings/oauth_popup.html`), then run
//! the same dialog-find + label-walk-up read that
//! post_create_dialog uses for the first three credentials.
//! Helpers (`direct_text`, `find_value_for_label`, …) are
//! re-exported from that module.

use std::fs;
use std::path::PathBuf;

use psychological_operations_browser_sdk::mode::Mode;
use scraper::{ElementRef, Html, Selector};
use tauri::{AppHandle, Wry};

use crate::post_create_dialog::{collect_text, find_value_for_label};
use crate::webview;

#[derive(Debug, Default, Clone)]
pub struct ExtractedOAuthCredentials {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// Save the popup's full document HTML for offline inspection
/// — overwritten on each invocation. Always called *before*
/// `extract` so a parse failure still leaves the fixture
/// behind.
pub fn save_snapshot(app: &AppHandle<Wry>, html: &str) -> std::io::Result<PathBuf> {
    let dir = webview::mode_data_dir(app, &Mode::XApp).join("recordings");
    fs::create_dir_all(&dir)?;
    let path = dir.join("oauth_popup.html");
    fs::write(&path, html.as_bytes())?;
    Ok(path)
}

const FIELDS: [(&str, &str); 2] = [
    ("client_id", "Client ID"),
    ("client_secret", "Client Secret"),
];

/// Parse a full document HTML string. Strategy mirrors
/// `post_create_dialog::extract`: find the dialog whose
/// visible text contains both labels, fall back to any
/// `[role="dialog"]` on miss, then per-label walk-up to
/// locate the value-bearing element.
pub fn extract(html: &str) -> ExtractedOAuthCredentials {
    let doc = Html::parse_document(html);

    let dialog_sel = Selector::parse(r#"[role="dialog"]"#).expect("valid selector");
    let dialog = doc
        .select(&dialog_sel)
        .find(|d| dialog_has_all_labels(*d))
        .or_else(|| doc.select(&dialog_sel).next());

    let Some(dialog) = dialog else {
        return ExtractedOAuthCredentials::default();
    };

    let mut out = ExtractedOAuthCredentials::default();
    for (key, label) in FIELDS {
        let value = find_value_for_label(dialog, label);
        match key {
            "client_id" => out.client_id = value,
            "client_secret" => out.client_secret = value,
            _ => {}
        }
    }
    out
}

fn dialog_has_all_labels(dialog: ElementRef<'_>) -> bool {
    let text = collect_text(dialog).to_lowercase();
    FIELDS
        .iter()
        .all(|(_, label)| text.contains(&label.to_lowercase()))
}
