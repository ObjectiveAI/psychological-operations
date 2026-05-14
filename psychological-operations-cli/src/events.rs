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
//! // ScoringComplete { psyop: "demo", scored: 5, survivors: 3, stages: 2 }
//! {"type":"notification",
//!  "value":{"event":"scoring_complete","psyop":"demo","scored":5,"survivors":3,"stages":2}}
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
    FilterComplete {
        psyop: String,
        accepted: usize,
        min_posts: u64,
        max_posts: u64,
    },
    PostsHydrated   { psyop: String, count: usize },
    HydratingQueue  { psyop: String, count: usize },
    StageEmpty      { psyop: String, stage: usize },
    ScoringStarted  { count: usize },
    ScoringComplete {
        psyop: String,
        scored: usize,
        survivors: usize,
        stages: usize,
    },
    ContentsDropped  { psyop: String, count: usize },
    DeliveryComplete {
        psyop: String,
        delivered: usize,
        failed: usize,
    },

    // ── query / ingest ───────────────────────────────────────
    QuerySkipped  { psyop: String, query: String, reason: String },
    QueryComplete { psyop: String, query: String, count: usize },

    // ── browse / chromium ────────────────────────────────────
    BrowseChromiumMaterialized { path: String },
    BrowseNoPsyops,
    BrowsePsyopList            { count: usize },
    BrowseStarting             {
        psyop: String,
        commit: String,
        index: usize,
        total: usize,
    },
    BrowseChromiumExit         { psyop: String, status: Option<i32> },
    ChromiumSpawned            { psyop: String, commit: String, pid: u32 },

    // ── oauth / setup ────────────────────────────────────────
    OauthListening   { psyop: String, port: u16, timeout_secs: u64 },
    OauthTokensSaved { psyop: String, scope: String, expires_at: String },
    XAppSetupInstructions {
        profile: String,
        child_pid: u32,
        instructions: String,
    },

    // ── target delivery ──────────────────────────────────────
    TargetDelivered { body: serde_json::Value },

    // ── error-flavored variants (routed through emit_error) ──
    ObjectiveaiTaskErrors { count: usize },
    TweetNotFound         { psyop: String, tweet_id: String },
    TweetFetchFailed      { psyop: String, tweet_id: String, error: String },
    QueryFailed           { psyop: String, query: String, error: String },
    DeliveryFailed        { delivery_id: i64, reason: String },
}

impl Event {
    /// `Some(level)` ⇒ this event is a failure / warning and should
    /// be emitted as `PluginOutput::Error(level, fatal=false, …)`.
    /// `None` ⇒ informational, emit as `PluginOutput::Notification`.
    pub(crate) fn error_level(&self) -> Option<objectiveai_cli_sdk::output::Level> {
        use objectiveai_cli_sdk::output::Level;
        match self {
            Event::ObjectiveaiTaskErrors { .. }
            | Event::TweetNotFound { .. }
            | Event::TweetFetchFailed { .. }
            | Event::QueryFailed { .. }
            | Event::DeliveryFailed { .. } => Some(Level::Warn),
            _ => None,
        }
    }
}
