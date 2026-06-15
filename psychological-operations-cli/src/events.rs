//! Catalog of every PluginOutput notification / error this plugin
//! emits.
//!
//! Each variant carries the fields a structured consumer needs;
//! `serde` derives the wire form via internal tagging on `event`,
//! which lands inside the `value` field of the host's outer
//! `PluginOutput::Notification` (or as the structured `message` of
//! `PluginOutput::Error` for the failure-flavored variants — see
//! [`Event::error_level`]).
//!
//! Wire shape (Notification example):
//!
//! ```jsonc
//! // StageBegin { stage: 0 }
//! {"type":"notification","value":{"event":"stage_begin","stage":0}}
//! ```
//!
//! Wire shape (Error example):
//!
//! ```jsonc
//! // DeliveryFailed { delivery_id: 7, reason: "timeout" }
//! {"type":"error","level":"warn","fatal":false,
//!  "message":{"event":"delivery_failed","delivery_id":7,"reason":"timeout"}}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    // ── lifecycle markers ────────────────────────────────────
    StageBegin { stage: usize },
    StageEnd   { stage: usize },

    // ── psyop run pipeline ───────────────────────────────────
    HydratingQueue { psyop: String, count: usize },
    StageEmpty     { psyop: String, stage: usize },

    // ── query / ingest ───────────────────────────────────────
    QuerySkipped  { psyop: String, query: String, reason: String },
    QueryComplete { psyop: String, query: String, count: usize },

    // ── browse / browser ─────────────────────────────────────
    BrowseBrowserMaterialized { path: String },
    BrowseSessionEnded {
        psyop: String,
        status: Option<i32>,
        inserted: usize,
        skipped: usize,
    },
    /// The embedded browser subprocess was launched. `kind` is one
    /// of `"x_app"`, `"psyop_read"`, `"psyop_authorize"`,
    /// `"agent_authorize"`. `name` carries the psyop/agent name
    /// for everything but `x_app`.
    BrowserSpawned {
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        pid: u32,
    },
    /// The embedded browser subprocess exited.
    BrowserExit {
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        status: Option<i32>,
    },

    // ── target delivery ──────────────────────────────────────
    TargetDelivered { body: serde_json::Value },

    // ── error-flavored variants (routed through emit_error) ──
    ObjectiveaiTaskErrors { count: usize },
    TweetNotFound         { psyop: String, tweet_id: String },
    TweetFetchFailed      { psyop: String, tweet_id: String, error: String },
    QueryFailed           { psyop: String, query: String, error: String },
    DeliveryFailed        { delivery_id: i64, reason: String },
    /// The browser child wrote an `{"error": ...}` line on its piped
    /// stdout while we were streaming for a terminator. Non-fatal:
    /// the read loop keeps going.
    BrowserError          { error: String },
    /// `psyops run` loaded a psyop that failed `validate()`. The
    /// run is skipped (exit code stays 0); the operator sees this
    /// warning event with the reason.
    PsyopInvalidAtRun     { psyop: String, reason: String },
    /// `psyops run` was invoked before the psyop's `interval` had
    /// elapsed since its last successful run. The run is skipped
    /// (exit code stays 0); `remaining_secs` is how much longer
    /// the operator has to wait.
    PsyopSkippedInterval  { psyop: String, interval: String, remaining_secs: u64 },
    /// One psyop in a `psyops run` batch hit a hard error (failed to
    /// load, X-App not set up, `for_you` collection failed, or the run
    /// itself errored). Emitted per-psyop so it doesn't abort the other
    /// psyops; the command exit code stays 0 (only db/infra errors are
    /// fatal).
    PsyopRunFailed        { psyop: String, error: String },
}

impl Event {
    /// `Some(level)` ⇒ this event is a failure / warning and should
    /// be emitted as the `OutputResult::Error` variant.
    /// `None` ⇒ informational, emit as `OutputResult::Notification`.
    pub(crate) fn error_level(&self) -> Option<objectiveai_sdk::cli::Level> {
        use objectiveai_sdk::cli::Level;
        match self {
            Event::ObjectiveaiTaskErrors { .. }
            | Event::TweetNotFound { .. }
            | Event::TweetFetchFailed { .. }
            | Event::QueryFailed { .. }
            | Event::DeliveryFailed { .. }
            | Event::BrowserError { .. }
            | Event::PsyopInvalidAtRun { .. }
            | Event::PsyopSkippedInterval { .. }
            | Event::PsyopRunFailed { .. } => Some(Level::Warn),
            _ => None,
        }
    }
}
