//! Parses X's post-app-create dialog HTML and extracts the three
//! credentials it surfaces (Consumer Key, Secret Key, Bearer
//! Token). Runs against a full document HTML string the overlay
//! ships in via the `process_post_create_html` Tauri command.
//!
//! Every invocation also snapshots the raw HTML to
//! `<x-app-data-dir>/recordings/post_create_dialog.html`
//! (overwriting) so we can iterate selectors against real markup
//! without needing a live browser session — open the snapshot in
//! any editor and tune.
//!
//! The overlay does zero parsing here; this module owns it all.

use std::fs;
use std::path::PathBuf;

use psychological_operations_browser_sdk::mode::Mode;
use scraper::{ElementRef, Html, Selector};
use tauri::{AppHandle, Wry};

use crate::webview;

/// Three credentials this module looks for. Each is `None` if
/// the parse couldn't locate it (markup change, value not
/// rendered yet, etc.); the Tauri command treats partial results
/// as "still working on it" — the overlay keeps re-sending HTML
/// every few seconds until all three land.
#[derive(Debug, Default, Clone)]
pub struct ExtractedCredentials {
    pub consumer_key: Option<String>,
    pub secret_key: Option<String>,
    pub bearer_token: Option<String>,
}

/// Persist the latest HTML payload to disk for inspection. Always
/// called *before* `extract`, so a parse failure still leaves the
/// snapshot behind for selector refinement.
pub fn save_snapshot(app: &AppHandle<Wry>, html: &str) -> std::io::Result<PathBuf> {
    // Always under X-App's data dir — the post-create dialog only
    // exists on the X developer console.
    let dir = webview::mode_data_dir(app, &Mode::XApp).join("recordings");
    fs::create_dir_all(&dir)?;
    let path = dir.join("post_create_dialog.html");
    fs::write(&path, html.as_bytes())?;
    Ok(path)
}

const FIELDS: [(&str, &str); 3] = [
    ("consumer_key", "Consumer Key"),
    ("secret_key", "Secret Key"),
    ("bearer_token", "Bearer Token"),
];

/// Parse a full document HTML string. Returns whichever
/// credentials we could locate. Strategy:
///
///   1. Find the first `[role="dialog"]` whose visible text
///      contains all three field labels.
///   2. For each field, find the first descendant element whose
///      direct text (trimmed) matches the label, then walk up
///      a few ancestors looking for an input / textarea / code
///      / pre / span carrying a value that doesn't itself
///      match the label.
///
/// Defensive: each step falls through silently on miss — partial
/// results are normal until the user clicks "reveal" on any
/// masked field or the page finishes rendering.
pub fn extract(html: &str) -> ExtractedCredentials {
    let doc = Html::parse_document(html);

    let dialog_sel = Selector::parse(r#"[role="dialog"]"#).expect("valid selector");
    let dialog = doc
        .select(&dialog_sel)
        .find(|d| dialog_has_all_labels(*d))
        .or_else(|| {
            // Fallback: first dialog at all.
            doc.select(&dialog_sel).next()
        });

    let Some(dialog) = dialog else {
        return ExtractedCredentials::default();
    };

    let mut out = ExtractedCredentials::default();
    for (key, label) in FIELDS {
        let value = find_value_for_label(dialog, label);
        match key {
            "consumer_key" => out.consumer_key = value,
            "secret_key" => out.secret_key = value,
            "bearer_token" => out.bearer_token = value,
            _ => {}
        }
    }
    out
}

fn dialog_has_all_labels(dialog: ElementRef<'_>) -> bool {
    let text = collect_text(dialog).to_lowercase();
    FIELDS.iter().all(|(_, label)| text.contains(&label.to_lowercase()))
}

fn collect_text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ")
}

/// Direct text content of an element (only its own text nodes —
/// no descendants). Used to match label-bearing elements
/// precisely.
fn direct_text(el: ElementRef<'_>) -> String {
    use scraper::Node;
    let mut buf = String::new();
    for child in el.children() {
        if let Node::Text(t) = child.value() {
            buf.push_str(t);
        }
    }
    buf.trim().to_string()
}

/// For a given label string, find its value in the dialog tree.
/// Walks every descendant looking for one whose *direct* text
/// matches the label (case-insensitive), then climbs ancestors
/// to find a nearby input/code/textarea/pre/span with a value.
fn find_value_for_label(dialog: ElementRef<'_>, label: &str) -> Option<String> {
    let label_lower = label.to_lowercase();
    let all_sel = Selector::parse("*").expect("valid selector");

    for el in dialog.select(&all_sel) {
        let direct = direct_text(el).to_lowercase();
        if direct != label_lower {
            continue;
        }

        // Look for a value near this label element. Strategy:
        // climb up to N ancestors and at each level scan
        // descendants for a value-carrying element. First
        // non-empty match wins.
        let mut current = Some(el);
        for _ in 0..5 {
            let Some(node) = current else { break };
            if let Some(v) = scan_for_value(node, label) {
                return Some(v);
            }
            current = node.parent().and_then(ElementRef::wrap);
        }
    }
    None
}

/// Look inside `el`'s descendants for an input/textarea/code/pre
/// carrying a credential-looking value. Skips values that are
/// just the label text itself (defensive).
fn scan_for_value(el: ElementRef<'_>, label: &str) -> Option<String> {
    let label_lower = label.to_lowercase();
    let input_sel =
        Selector::parse("input, textarea, code, pre").expect("valid selector");
    for cand in el.select(&input_sel) {
        let value = match cand.value().name() {
            "input" | "textarea" => cand
                .value()
                .attr("value")
                .map(str::to_string)
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    let t = collect_text(cand).trim().to_string();
                    if t.is_empty() { None } else { Some(t) }
                }),
            _ => {
                let t = collect_text(cand).trim().to_string();
                if t.is_empty() { None } else { Some(t) }
            }
        };
        if let Some(v) = value {
            // Skip if the value is just the label echoed.
            if v.to_lowercase() == label_lower {
                continue;
            }
            // Skip obvious mask markers ("•••" / "***" / "xxxx").
            if is_likely_mask(&v) {
                continue;
            }
            return Some(v);
        }
    }
    None
}

fn is_likely_mask(s: &str) -> bool {
    s.chars().all(|c| c == '•' || c == '*' || c == 'x' || c == 'X' || c == '·')
}
