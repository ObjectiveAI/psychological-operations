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
//! // QueryFailed { psyop: "p", query: "q", error: "timeout" }
//! {"type":"error","level":"warn","fatal":false,
//!  "message":{"event":"query_failed","psyop":"p","query":"q","error":"timeout"}}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    // ── lifecycle markers ────────────────────────────────────
    StageBegin {
        stage: usize,
    },
    StageEnd {
        stage: usize,
    },

    // ── psyop run pipeline ───────────────────────────────────
    /// Hydrating an agent's collected For You tweet IDs via the X API.
    /// Per-agent (collection is shared across psyops), so keyed by agent.
    HydratingQueue {
        agent: String,
        count: usize,
    },
    StageEmpty {
        psyop: String,
        stage: usize,
    },

    // ── query / ingest ───────────────────────────────────────
    QueryComplete {
        psyop: String,
        query: String,
        count: usize,
    },

    // ── browse / browser ─────────────────────────────────────
    BrowseBrowserMaterialized {
        path: String,
    },
    /// An agent's For You collection browser session ended. `collected`
    /// is the count of distinct tweet IDs streamed before the operator
    /// closed the window.
    BrowseSessionEnded {
        agent: String,
        status: Option<i32>,
        collected: usize,
    },
    /// The embedded browser subprocess was launched. `kind` is one
    /// of `"x_app"`, `"agent_read"`, `"agent_authorize"`,
    /// `"agent_browser"`. `name` carries the agent tag for
    /// everything but `x_app`.
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
    /// One queued reply/quote was delivered by the browser (in
    /// `agents deliver`); the matching `reply_quote_queue` row has been
    /// removed. Informational per-item progress.
    Delivered {
        tweet_id: String,
        agent: String,
        kind: String,
    },

    // ── error-flavored variants (routed through emit_error) ──
    ObjectiveaiTaskErrors {
        count: usize,
    },
    TweetNotFound {
        agent: String,
        tweet_id: String,
    },
    TweetFetchFailed {
        agent: String,
        tweet_id: String,
        error: String,
    },
    QueryFailed {
        psyop: String,
        query: String,
        error: String,
    },
    /// The browser child wrote an `{"error": ...}` line on its piped
    /// stdout while we were streaming for a terminator. Non-fatal:
    /// the read loop keeps going.
    BrowserError {
        error: String,
    },
    /// `psyops run` loaded a psyop that failed `validate()`. The
    /// run is skipped (exit code stays 0); the operator sees this
    /// warning event with the reason.
    PsyopInvalidAtRun {
        psyop: String,
        reason: String,
    },
    /// `psyops run` was invoked before the psyop's `interval` had
    /// elapsed since its last successful run. The run is skipped
    /// (exit code stays 0); `remaining_secs` is how much longer
    /// the operator has to wait.
    PsyopSkippedInterval {
        psyop: String,
        interval: String,
        remaining_secs: u64,
    },
    /// One psyop in a `psyops run` batch hit a hard error (failed to
    /// load, X-App not set up, `for_you` collection failed, or the run
    /// itself errored). Emitted per-psyop so it doesn't abort the other
    /// psyops; the command exit code stays 0 (only db/infra errors are
    /// fatal).
    PsyopRunFailed {
        psyop: String,
        error: String,
    },
    /// `psyops run` found that an `agent_tag` referenced by a psyop's
    /// queries / for_you has no valid auth (never logged in, or no
    /// tokens). The psyop is skipped (exit code stays 0); the operator
    /// must log the agent in.
    PsyopAgentNotAuthed {
        psyop: String,
        agent_tag: String,
    },
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
            | Event::BrowserError { .. }
            | Event::PsyopInvalidAtRun { .. }
            | Event::PsyopSkippedInterval { .. }
            | Event::PsyopRunFailed { .. }
            | Event::PsyopAgentNotAuthed { .. } => Some(Level::Warn),
            _ => None,
        }
    }
}
