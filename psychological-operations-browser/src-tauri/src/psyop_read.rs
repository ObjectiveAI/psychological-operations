//! In-memory deduper + emitter for [`Mode::PsyopRead`].
//!
//! The overlay (see `src/overlay/psyop-read-helpers.ts`)
//! periodically ships `document.documentElement.outerHTML` to
//! Rust via the CEF custom-scheme `process_read_html` route.
//! Each call lands here in [`process_html`], which:
//!
//!   1. Bails when the twid-conflict guard is active — we
//!      don't want the wrong account's timeline polluting the
//!      seen set.
//!   2. On the *first* call of a session, writes a copy of the
//!      raw HTML to
//!      `<psyop-data-dir>/handles/<twid>/recordings/timeline.html`
//!      so we have a real fixture to design the tweet-ID
//!      parser against. (One-shot per session; clobbers any
//!      prior snapshot from the same psyop+twid.)
//!   3. Parses tweet IDs out of the HTML
//!      ([`parse_tweet_ids`] — currently stubbed to return
//!      empty; real parser is a follow-up).
//!   4. For each *new* ID, pushes onto the ordered list and
//!      emits an `Output::TweetId { id }` line on stdout.
//!   5. Updates [`crate::state::set_tweets_read_count`] so
//!      the panel's "Tweets read: X" counter advances.
//!
//! The seen set is in-memory only and lives in a
//! `OnceLock<Mutex<Seen>>`. [`clear`] zeroes it, called from
//! `state::set_mode` on every mode flip (including psyop
//! swap) so a fresh session always starts at zero.

use std::collections::HashSet;
use std::fs;
use std::sync::{Mutex, OnceLock};

use psychological_operations_browser_sdk::mode::Mode;
use psychological_operations_browser_sdk::output::Output;
use tauri::{AppHandle, Wry};

use crate::state;
use crate::webview;

#[derive(Default)]
struct Seen {
    /// Insertion-order list of distinct IDs. The size of this
    /// vec is what the panel counter reports.
    ids: Vec<String>,
    /// Membership set, populated in lockstep with `ids`.
    set: HashSet<String>,
    /// True once we've written the first-HTML snapshot to
    /// disk. Resets on [`clear`] alongside the rest.
    snapshot_written: bool,
}

fn seen_slot() -> &'static Mutex<Seen> {
    static SLOT: OnceLock<Mutex<Seen>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Seen::default()))
}

/// Called from `state::set_mode` whenever the mode changes
/// (including psyop swap). Drops every accumulated ID so the
/// next session's counter starts at zero.
pub fn clear() {
    if let Ok(mut seen) = seen_slot().lock() {
        *seen = Seen::default();
    }
}

/// Entry point for the CEF `process_read_html` custom-scheme
/// command. Returns the current session's tweet count after
/// processing — the overlay uses it as a back-pressure
/// signal (it just acks; the panel's `Output::Panel` carries
/// the same count to any host process listening).
pub fn process_html(handle: &AppHandle<Wry>, html: String) -> u32 {
    // Guard: only meaningful in PsyopRead mode. A late HTML
    // invoke from the prior overlay during a mode swap could
    // land after `state::set_mode` flipped the mode — drop it.
    let psyop_name = match psychological_operations_browser_sdk::mode::get() {
        Some(Mode::PsyopRead { name }) => name,
        _ => return current_count(),
    };

    // Don't ingest a wrong-account timeline into the seen set.
    // The panel surfaces the conflict separately; we just
    // skip until the user signs back in correctly.
    if state::twid_conflict_present() {
        return current_count();
    }

    let twid = match state::current_user_id() {
        Some(t) => t,
        None => return current_count(),
    };

    write_first_snapshot(handle, &psyop_name, &twid, &html);

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

/// Tweet-ID extractor. **DEFERRED** — returns empty until we
/// design the real selector against a captured timeline.html
/// fixture. The first call to [`process_html`] writes that
/// fixture to disk; once we have it, fill in the body here
/// (likely a `scraper::Selector` over the tweet article DOM).
fn parse_tweet_ids(_html: &str) -> Vec<String> {
    Vec::new()
}

/// One-shot per session: drop a copy of the raw HTML to
/// `<psyop-data-dir>/handles/<twid>/recordings/timeline.html`
/// so we have a real fixture to build the parser against.
/// Errors are logged but don't fail the processing path —
/// snapshot is dev-affordance, not load-bearing.
fn write_first_snapshot(
    handle: &AppHandle<Wry>,
    psyop_name: &str,
    twid: &str,
    html: &str,
) {
    {
        let seen = match seen_slot().lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        if seen.snapshot_written {
            return;
        }
    }
    let mode = Mode::PsyopRead {
        name: psyop_name.to_string(),
    };
    let path = webview::mode_data_dir(handle, &mode)
        .join("handles")
        .join(twid)
        .join("recordings")
        .join("timeline.html");
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            let _ = Output::Log {
                message: format!(
                    "psyop_read: mkdir {}: {e}",
                    parent.display()
                ),
            }
            .emit();
            return;
        }
    }
    if let Err(e) = fs::write(&path, html) {
        let _ = Output::Log {
            message: format!("psyop_read: write {}: {e}", path.display()),
        }
        .emit();
        return;
    }
    if let Ok(mut seen) = seen_slot().lock() {
        seen.snapshot_written = true;
    }
}
