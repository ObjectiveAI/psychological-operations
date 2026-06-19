//! In-memory deduper + emitter for [`Mode::AgentRead`].
//!
//! The overlay (see `src/overlay/agent-read-helpers.ts`)
//! periodically ships `document.documentElement.outerHTML` to
//! Rust via the CEF custom-scheme `process_read_html` route.
//! Each call lands here in [`process_html`], which:
//!
//!   1. Parses tweet IDs out of the HTML via
//!      [`parse_tweet_ids`] — walks
//!      `article[data-testid="tweet"]` elements and picks
//!      each one's first `/status/<digits>` descendant URL.
//!   2. For each *new* ID, pushes onto the ordered list and
//!      emits an `Output::TweetId { id }` line on stdout.
//!   3. Updates [`crate::state::set_tweets_read_count`] so
//!      the panel's "Tweets read: X" counter advances.
//!
//! The seen set is in-memory only and lives in a
//! `OnceLock<Mutex<Seen>>`. Mode is locked at the process's
//! CLI flag so the set never needs zeroing — the process
//! lives and dies with one mode.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use psychological_operations_sdk::browser::output::Output;
use scraper::{Html, Selector};
use tauri::{AppHandle, Wry};

use crate::state;

#[derive(Default)]
struct Seen {
    /// Insertion-order list of distinct IDs. The size of this
    /// vec is what the panel counter reports.
    ids: Vec<String>,
    /// Membership set, populated in lockstep with `ids`.
    set: HashSet<String>,
}

fn seen_slot() -> &'static Mutex<Seen> {
    static SLOT: OnceLock<Mutex<Seen>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Seen::default()))
}

/// Entry point for the CEF `process_read_html` custom-scheme
/// command. Returns the current session's tweet count after
/// processing — the overlay uses it as a back-pressure
/// signal (it just acks; the panel's `Output::Panel` carries
/// the same count to any host process listening).
pub fn process_html(handle: &AppHandle<Wry>, html: String) -> u32 {
    let parsed = parse_tweet_ids(&html);
    let new_count = {
        let mut seen = match seen_slot().lock() {
            Ok(s) => s,
            Err(_) => return current_count(),
        };
        for id in parsed {
            if seen.set.insert(id.clone()) {
                seen.ids.push(id.clone());
                let _ = Output::TweetId { id }.emit();
            }
        }
        seen.ids.len() as u32
    };

    state::set_tweets_read_count(handle, new_count);
    new_count
}

fn current_count() -> u32 {
    seen_slot()
        .lock()
        .map(|s| s.ids.len() as u32)
        .unwrap_or(0)
}

/// Extract tweet IDs from the For-You feed HTML.
///
/// Walks every `<article data-testid="tweet">` (one per
/// feed item) and picks the first `<a href="…/status/<id>…">`
/// inside it. The first `/status/` URL per article is always
/// the wrapper tweet's own ID; subsequent ones are media
/// affordances (`/status/<id>/photo/1`,
/// `/status/<id>/retweets`, …) or — for quote-tweets — the
/// quoted inner article, which we deliberately drop here.
///
/// Returns IDs in document order (= feed order).
fn parse_tweet_ids(html: &str) -> Vec<String> {
    static ARTICLE_SEL: OnceLock<Selector> = OnceLock::new();
    static LINK_SEL: OnceLock<Selector> = OnceLock::new();
    let article_sel = ARTICLE_SEL.get_or_init(|| {
        Selector::parse(r#"article[data-testid="tweet"]"#).expect("article selector")
    });
    let link_sel = LINK_SEL
        .get_or_init(|| Selector::parse(r#"a[href*="/status/"]"#).expect("link selector"));

    let doc = Html::parse_document(html);
    let mut ids = Vec::new();
    for article in doc.select(article_sel) {
        for link in article.select(link_sel) {
            if let Some(href) = link.value().attr("href") {
                if let Some(id) = extract_status_id(href) {
                    ids.push(id);
                    break;
                }
            }
        }
    }
    ids
}

/// Pull the numeric tweet ID out of a `/status/<id>…` URL.
///
/// Accepts the bare-path form (`/handle/status/123…`) that x.com
/// emits inline. Uses the *rightmost* `/status/` segment so a
/// hypothetical future href like `/i/status/<x>/status/<y>` would
/// resolve to the inner ID; in practice x.com's anchors are
/// always the simple form.
fn extract_status_id(href: &str) -> Option<String> {
    let after = href.rfind("/status/")? + "/status/".len();
    let id: String = href[after..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if id.is_empty() { None } else { Some(id) }
}
