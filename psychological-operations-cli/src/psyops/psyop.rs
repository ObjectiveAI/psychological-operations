use serde::{Deserialize, Serialize};
use objectiveai::functions::{
    FullInlineFunctionOrRemoteCommitOptional,
    FullInlineFunction,
    AlphaInlineFunction,
    InlineFunction,
    InlineProfileOrRemoteCommitOptional,
};
use objectiveai::functions::executions::request::Strategy;

use super::for_you::ForYou;
use super::query::Query;
use super::sort::SortBy;

/// A psyop scores tweets pulled from one or more X v2 sources. Output
/// scores get persisted with `tags` so external consumers can select
/// them by tag. At least one of `queries` or `for_you` must be set.
#[derive(Debug, Serialize, Deserialize)]
pub struct PsyOp {
    /// Live X v2 search-query inputs. `None` means no query-driven
    /// ingestion for this psyop. An empty `Some(vec![])` is equivalent
    /// to `None` for ingestion purposes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queries: Option<Vec<Query>>,
    /// Personalized "For You" timeline input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_you: Option<ForYou>,

    /// Tags applied to every score row this psyop produces.
    pub tags: Vec<String>,

    pub function: FullInlineFunctionOrRemoteCommitOptional,
    pub profile: InlineProfileOrRemoteCommitOptional,
    pub strategy: Strategy,

    #[serde(default)]
    pub invert: bool,
    /// If `false`, scored posts are sent to the function with an empty
    /// `images` array regardless of what was ingested. Defaults to `true`.
    #[serde(default = "default_true")]
    pub images: bool,
    /// If `false`, scored posts are sent to the function with an empty
    /// `videos` array regardless of what was ingested. Defaults to `true`.
    #[serde(default = "default_true")]
    pub videos: bool,

    /// Minimum total deduped candidates required before the psyop will
    /// run scoring. If the union of `queries` + `for_you` falls below
    /// this, the psyop is skipped.
    pub min_posts: u64,
    /// Hard cap on candidates sent to the scoring function. After
    /// dedup, the candidate set is ordered by `(priority, sort)` and
    /// truncated to `max_posts`.
    pub max_posts: u64,

    /// Tiebreak ordering applied across the deduped candidate union.
    /// Combines with per-source `priority` (priority is primary,
    /// descending; `None` ranks below every `Some(_)`. `sort` is the
    /// tiebreak among equal-priority items).
    pub sort: SortBy,
}

fn default_true() -> bool { true }

/// Read a psyop's JSON definition from disk.
pub fn load(name: &str) -> Result<PsyOp, crate::error::Error> {
    let path = crate::config::psyops_dir().join(name).join("psyop.json");
    if !path.exists() {
        return Err(crate::error::Error::PsyopNotFound(path.display().to_string()));
    }
    let data = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&data)?)
}

/// Write a psyop's JSON definition back to disk (pretty-printed).
pub fn save(name: &str, psyop: &PsyOp) -> Result<(), crate::error::Error> {
    let path = crate::config::psyops_dir().join(name).join("psyop.json");
    let json = serde_json::to_string_pretty(psyop)?;
    std::fs::write(&path, json + "\n")?;
    Ok(())
}

impl PsyOp {
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        let has_queries = self.queries.as_ref().is_some_and(|v| !v.is_empty());
        let has_for_you = self.for_you.is_some();
        if !has_queries && !has_for_you {
            return Err(crate::error::Error::InvalidPsyop(
                "psyop must have at least one of `queries` or `for_you`".into(),
            ));
        }
        if self.tags.is_empty() {
            return Err(crate::error::Error::InvalidPsyop("tags must not be empty".into()));
        }
        if self.max_posts == 0 {
            return Err(crate::error::Error::InvalidPsyop("max_posts must be > 0".into()));
        }
        if self.min_posts > self.max_posts {
            return Err(crate::error::Error::InvalidPsyop(
                "min_posts must be <= max_posts".into(),
            ));
        }
        if let Some(qs) = &self.queries {
            for (i, q) in qs.iter().enumerate() {
                if q.query.trim().is_empty() {
                    return Err(crate::error::Error::InvalidPsyop(
                        format!("queries[{i}]: query string must not be empty"),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Determine if a function is a vector function.
/// If the function is remote, it must be fetched first (caller resolves it).
pub fn is_vector_function(function: &FullInlineFunction) -> bool {
    match function {
        FullInlineFunction::Alpha(alpha) => matches!(alpha, AlphaInlineFunction::Vector(_)),
        FullInlineFunction::Standard(standard) => matches!(standard, InlineFunction::Vector { .. }),
    }
}
