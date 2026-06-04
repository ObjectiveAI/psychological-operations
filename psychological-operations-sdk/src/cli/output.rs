//! Success-side output of a `psychological-operations` CLI
//! invocation. Errors go through a separate channel
//! (`objectiveai_sdk::cli::Error` at the host boundary).

use serde::{Deserialize, Serialize};

/// What a CLI command emits on the happy path. Wire form is
/// untagged-style — each variant serializes to the value its
/// payload carries (or omits itself when empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    /// Result of `... config get …` — JSON / opaque string the
    /// host renders verbatim.
    ConfigGet(String),
    /// Result of `... config set …` — no body.
    ConfigSet,
    /// Generic API-shaped scalar result (e.g. a sha, a single id).
    Api(String),
    /// Command produced nothing to emit.
    Empty,
}

impl std::fmt::Display for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Output::ConfigGet(s) => write!(f, "{s}"),
            Output::ConfigSet => write!(f, "ok"),
            Output::Api(s) => write!(f, "{s}"),
            Output::Empty => Ok(()),
        }
    }
}
