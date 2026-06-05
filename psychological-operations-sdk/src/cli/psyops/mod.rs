//! The `PsyOp` type graph + publish-time validators — shared
//! body shape for `psychological-operations psyops publish
//! --psyop-inline '<json>'`.
//!
//! Pure data + `validate()` methods. Runtime concerns (git2 disk
//! I/O for load/save, Starlark evaluation against live `Tweet`
//! rows) live in the CLI alongside the scoring pipeline.

pub mod filter;
pub mod for_you;
pub mod psyop;
pub mod query;
pub mod sort_by;
pub mod stage;

pub use filter::Filter;
pub use for_you::ForYou;
pub use psyop::PsyOp;
pub use query::{Query, SearchEndpoint};
pub use sort_by::SortBy;
pub use stage::{is_vector_function, Stage};

use serde::{Deserialize, Serialize};

/// One row of `psyops list`. Resolved per name + HEAD commit +
/// config overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsyopEntry {
    pub name: String,
    pub enabled: bool,
    pub commit_sha: String,
}

/// Returned by `psyops publish` — captures both the just-
/// committed sha and the resolved enabled state at that sha.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedPsyop {
    pub name: String,
    pub commit_sha: String,
    pub enabled: bool,
}
