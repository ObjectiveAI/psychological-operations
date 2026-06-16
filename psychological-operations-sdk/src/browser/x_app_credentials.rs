//! Parsers for the two X developer-console HTML surfaces that
//! surface X-App credentials: the post-create dialog (Consumer Key,
//! Secret Key, Bearer Token) and the OAuth 2.0 settings popup
//! (Client ID, Client Secret).
//!
//! Both surfaces are captured as raw HTML snapshots on disk by the
//! browser crate; this module is the single shared parser every
//! downstream consumer (browser-side `psyop_authorize`, CLI, future
//! SDK users) goes through. Each struct carries every field as
//! `Option<String>` — partial parses are normal while the page is
//! mid-render or a value is still masked. Use
//! [`PostCreateDialog::is_complete`] / [`OAuthPopup::is_complete`]
//! to ask "did every field land".
//!
//! Snapshots are persisted in the db crate's `x_app_html` table, keyed
//! by `(handle, kind)` where `kind` is one of [`POST_CREATE_DIALOG_KIND`]
//! / [`OAUTH_POPUP_KIND`]. Consumers fetch + parse via the `from_db`
//! constructors; the browser writes via `Db::x_app_html_set`.

use psychological_operations_db::Db;
use scraper::{ElementRef, Html, Node, Selector};

/// `x_app_html.kind` value for the post-create dialog snapshot.
pub const POST_CREATE_DIALOG_KIND: &str = "post_create_dialog";
/// `x_app_html.kind` value for the OAuth 2.0 settings popup snapshot.
pub const OAUTH_POPUP_KIND: &str = "oauth_popup";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PostCreateDialog {
    pub consumer_key: Option<String>,
    pub secret_key: Option<String>,
    pub bearer_token: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OAuthPopup {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

impl PostCreateDialog {
    const FIELDS: [(&'static str, &'static str); 3] = [
        ("consumer_key", "Consumer Key"),
        ("secret_key", "Secret Key"),
        ("bearer_token", "Bearer Token"),
    ];

    /// In-memory parse. Always returns a struct; fields the dialog
    /// hasn't rendered yet stay `None`.
    pub fn parse(html: &str) -> Self {
        let doc = Html::parse_document(html);
        let labels: Vec<&str> = Self::FIELDS.iter().map(|(_, l)| *l).collect();
        let Some(dialog) = pick_dialog(&doc, &labels) else {
            return Self::default();
        };
        let mut out = Self::default();
        for (key, label) in Self::FIELDS {
            let value = find_value_for_label(dialog, label);
            match key {
                "consumer_key" => out.consumer_key = value,
                "secret_key" => out.secret_key = value,
                "bearer_token" => out.bearer_token = value,
                _ => unreachable!(),
            }
        }
        out
    }

    /// Fetch + parse the stored snapshot for `handle`. `Ok(None)` when
    /// no snapshot has been captured yet.
    pub async fn from_db(
        db: &Db,
        handle: &str,
    ) -> Result<Option<Self>, psychological_operations_db::Error> {
        Ok(db
            .x_app_html_get(handle, POST_CREATE_DIALOG_KIND)
            .await?
            .map(|html| Self::parse(&html)))
    }

    /// True iff all three fields parsed successfully.
    pub fn is_complete(&self) -> bool {
        self.consumer_key.is_some() && self.secret_key.is_some() && self.bearer_token.is_some()
    }

    /// Count of fields successfully parsed (0..=3). The browser
    /// returns this to the frontend as the green-state signal.
    pub fn parsed_count(&self) -> u8 {
        self.consumer_key.is_some() as u8
            + self.secret_key.is_some() as u8
            + self.bearer_token.is_some() as u8
    }
}

impl OAuthPopup {
    const FIELDS: [(&'static str, &'static str); 2] = [
        ("client_id", "Client ID"),
        ("client_secret", "Client Secret"),
    ];

    pub fn parse(html: &str) -> Self {
        let doc = Html::parse_document(html);
        let labels: Vec<&str> = Self::FIELDS.iter().map(|(_, l)| *l).collect();
        let Some(dialog) = pick_dialog(&doc, &labels) else {
            return Self::default();
        };
        let mut out = Self::default();
        for (key, label) in Self::FIELDS {
            let value = find_value_for_label(dialog, label);
            match key {
                "client_id" => out.client_id = value,
                "client_secret" => out.client_secret = value,
                _ => unreachable!(),
            }
        }
        out
    }

    /// Fetch + parse the stored snapshot for `handle`. `Ok(None)` when
    /// no snapshot has been captured yet.
    pub async fn from_db(
        db: &Db,
        handle: &str,
    ) -> Result<Option<Self>, psychological_operations_db::Error> {
        Ok(db
            .x_app_html_get(handle, OAUTH_POPUP_KIND)
            .await?
            .map(|html| Self::parse(&html)))
    }

    pub fn is_complete(&self) -> bool {
        self.client_id.is_some() && self.client_secret.is_some()
    }

    pub fn parsed_count(&self) -> u8 {
        self.client_id.is_some() as u8 + self.client_secret.is_some() as u8
    }
}

// ----- internal parsing helpers ---------------------------------

/// Pick the first `[role="dialog"]` whose visible text covers
/// every expected label; falls back to the first `[role="dialog"]`
/// otherwise so the per-label walk can still try.
fn pick_dialog<'a>(doc: &'a Html, labels: &[&str]) -> Option<ElementRef<'a>> {
    let sel = Selector::parse(r#"[role="dialog"]"#).expect("valid selector");
    doc.select(&sel)
        .find(|d| dialog_has_all_labels(*d, labels))
        .or_else(|| doc.select(&sel).next())
}

fn dialog_has_all_labels(dialog: ElementRef<'_>, labels: &[&str]) -> bool {
    let text = collect_text(dialog).to_lowercase();
    labels.iter().all(|l| text.contains(&l.to_lowercase()))
}

/// For a given label string, find its value in the dialog tree.
/// Walks every descendant looking for one whose *direct* text
/// matches the label (case-insensitive), then climbs ancestors
/// to find a nearby input / code / textarea / pre / monospace span
/// carrying a value.
fn find_value_for_label(dialog: ElementRef<'_>, label: &str) -> Option<String> {
    let label_lower = label.to_lowercase();
    let all_sel = Selector::parse("*").expect("valid selector");

    for el in dialog.select(&all_sel) {
        let direct = direct_text(el).to_lowercase();
        if direct != label_lower {
            continue;
        }

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

/// Look inside `el`'s descendants for an input / textarea / code /
/// pre / monospace `<p>` carrying a credential-looking value. Skips
/// values that are just the label text or an obvious mask
/// (`•••` / `***` / `xxxx`).
fn scan_for_value(el: ElementRef<'_>, label: &str) -> Option<String> {
    let label_lower = label.to_lowercase();
    let input_sel = Selector::parse(r#"input, textarea, code, pre, p[class*="font-mono"]"#)
        .expect("valid selector");
    for cand in el.select(&input_sel) {
        let value = match cand.value().name() {
            "input" | "textarea" => cand
                .value()
                .attr("value")
                .map(str::to_string)
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    let t = collect_text(cand).trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }),
            _ => {
                let t = collect_text(cand).trim().to_string();
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            }
        };
        if let Some(v) = value {
            if v.to_lowercase() == label_lower {
                continue;
            }
            if is_likely_mask(&v) {
                continue;
            }
            return Some(v);
        }
    }
    None
}

fn collect_text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ")
}

/// Only the element's own text nodes — no descendants. Used to
/// match label-bearing elements precisely.
fn direct_text(el: ElementRef<'_>) -> String {
    let mut buf = String::new();
    for child in el.children() {
        if let Node::Text(t) = child.value() {
            buf.push_str(t);
        }
    }
    buf.trim().to_string()
}

fn is_likely_mask(s: &str) -> bool {
    s.chars()
        .all(|c| c == '•' || c == '*' || c == 'x' || c == 'X' || c == '·')
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sanitized capture of the real X developer console post-create
    // dialog (May 2026 markup). Live secrets were replaced with
    // FIXTURE_* placeholders; structure is byte-identical otherwise.
    // Regenerate by pasting raw HTML from devtools (Elements panel
    // → right-click <html> → Copy outerHTML) into
    // `tests/fixtures/post_create_dialog.html`, then re-run the
    // redaction step described in the fixture-add commit message.
    const POST_CREATE_DIALOG_FIXTURE: &str =
        include_str!("../../tests/fixtures/post_create_dialog.html");

    #[test]
    fn extracts_all_three_credentials_from_real_markup() {
        let got = PostCreateDialog::parse(POST_CREATE_DIALOG_FIXTURE);
        assert_eq!(
            got.consumer_key.as_deref(),
            Some("FIXTURE_CONSUMER_KEY_VALUE"),
            "Consumer Key extraction regressed — X renders the value \
             in a <p class=\"font-mono ...\">, scan_for_value must \
             match that selector."
        );
        assert_eq!(got.secret_key.as_deref(), Some("FIXTURE_SECRET_KEY_VALUE"));
        assert_eq!(
            got.bearer_token.as_deref(),
            Some("FIXTURE_BEARER_TOKEN_VALUE"),
        );
        assert!(got.is_complete());
        assert_eq!(got.parsed_count(), 3);
    }

    #[test]
    fn empty_html_yields_empty_struct() {
        let got = PostCreateDialog::parse("<html></html>");
        assert!(!got.is_complete());
        assert_eq!(got.parsed_count(), 0);
        let got = OAuthPopup::parse("<html></html>");
        assert!(!got.is_complete());
        assert_eq!(got.parsed_count(), 0);
    }
}
