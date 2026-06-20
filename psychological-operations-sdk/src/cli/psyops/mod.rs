//! The `PsyOp` type graph + publish-time validators — shared
//! body shape for `psychological-operations psyops publish
//! --psyop-inline '<json>'`.
//!
//! Pure data + `validate()` methods. Runtime concerns (db-backed
//! load/save, Starlark evaluation against live `Tweet` rows) live in
//! the CLI alongside the scoring pipeline.

pub mod filter;
pub mod for_you;
pub mod mentions;
pub mod psyop;
pub mod query;
pub mod sort_by;
pub mod stage;
pub mod timeline;

pub use filter::Filter;
pub use for_you::ForYou;
pub use mentions::Mentions;
pub use psyop::PsyOp;
pub use query::Query;
pub use sort_by::SortBy;
pub use timeline::Timeline;
pub use stage::{is_vector_function, parse_output_top, OutputTop, Stage, StageBase};

use serde::{Deserialize, Serialize};

/// One row of `psyops list`. Resolved per name + its `disabled` flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsyopEntry {
    pub name: String,
    pub enabled: bool,
}

/// Returned by `psyops publish` — the upserted name + its resolved
/// enabled state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedPsyop {
    pub name: String,
    pub enabled: bool,
}
