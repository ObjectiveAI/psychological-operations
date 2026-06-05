//! Success-side output of a `psychological-operations` CLI
//! invocation. Errors go through a separate channel
//! (`objectiveai_sdk::cli::Error` at the host boundary).

use schemars::Schema;
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
    /// JSON Schema for an operator-facing input shape (e.g.
    /// `psyops schema` / `targets schema`). Carries the
    /// [`schemars::Schema`] value directly so consumers receive
    /// a structured object, not a stringified blob.
    Schema(Schema),
    /// Command produced nothing to emit.
    Empty,
}

impl std::fmt::Display for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Output::ConfigGet(s) => write!(f, "{s}"),
            Output::ConfigSet => write!(f, "ok"),
            Output::Api(s) => write!(f, "{s}"),
            Output::Schema(s) => write!(
                f,
                "{}",
                serde_json::to_string(s).expect("Schema serializes"),
            ),
            Output::Empty => Ok(()),
        }
    }
}
