use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::for_you::ForYou;
use super::query::Query;
use super::sort_by::SortBy;
use super::stage::Stage;

/// A psyop scores tweets pulled from one or more X v2 sources.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PsyOp {
    /// Live X v2 search-query inputs. `None` means no query-driven
    /// ingestion for this psyop. An empty `Some(vec![])` is equivalent
    /// to `None` for ingestion purposes; both round-trip out as
    /// "field absent" via `skip_queries`.
    #[serde(default, skip_serializing_if = "skip_queries")]
    pub queries: Option<Vec<Query>>,
    /// Personalized "For You" timeline input. `None` means no
    /// for-you ingestion for this psyop — the CLI runtime skips
    /// the queue check, the `hydrate_for_you` step, and the
    /// `query_when_for_you_queued` policy entirely. Publish-time
    /// validation requires at least one of `queries` (non-empty)
    /// or `for_you` to be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_you: Option<ForYou>,

    /// Minimum wall-clock time between runs, as a humantime duration
    /// string (e.g. `"1h 30m"`). `psyops run` records each psyop's
    /// last successful run and skips the psyop (exit 0, with a
    /// warning event) until the interval has elapsed. Validated at
    /// publish time via [`humantime::parse_duration`]; must be > 0.
    pub interval: String,

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

    /// When `false`, queries are skipped on a run as long as the
    /// for-you input still has queued candidates — the rationale
    /// being that if the algorithmic feed is feeding us enough
    /// material, paying for X v2 search calls is wasteful. When
    /// `true`, queries always run regardless of for-you queue state.
    /// Defaults to `true` (no implicit skipping).
    #[serde(default = "default_true")]
    pub query_when_for_you_queued: bool,

    /// Multi-stage scoring pipeline. Posts are scored by `stages[0]`,
    /// optionally narrowed via the stage's `output_threshold` /
    /// `output_top`, then fed to `stages[1]`, and so on. `None` or
    /// `Some(empty)` means no scoring — every survivor of the
    /// ingest → filter → sort → trim chain gets a max score (1.0)
    /// and flows through to delivery as-is. Per-stage threshold /
    /// top-N narrowing therefore doesn't apply when no stages are
    /// defined.
    #[serde(default, skip_serializing_if = "skip_stages")]
    pub stages: Option<Vec<Stage>>,
}

fn default_true() -> bool { true }

/// Skip-serializing predicate for `queries`: omit the field when
/// it's `None` OR `Some(empty)`. Both shapes mean "no query-driven
/// ingestion" so emitting `"queries": []` would just be noise.
fn skip_queries(q: &Option<Vec<Query>>) -> bool {
    match q {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Skip-serializing predicate for `stages`: omit the field when
/// it's `None` OR `Some(empty)`. Both shapes mean "no scoring";
/// the runtime synthesizes max-score survivors instead.
fn skip_stages(s: &Option<Vec<Stage>>) -> bool {
    match s {
        None => true,
        Some(v) => v.is_empty(),
    }
}

impl PsyOp {
    /// Publish-time consistency check. Returns a free-form error
    /// string; CLI callers wrap with their own `Error::InvalidPsyop`
    /// variant.
    /// Parsed form of [`interval`](Self::interval). `Err` carries
    /// the same message `validate()` rejects with — callers that
    /// have already validated can safely unwrap.
    pub fn interval_duration(&self) -> Result<std::time::Duration, String> {
        humantime::parse_duration(&self.interval)
            .map_err(|e| format!("interval: invalid humantime duration: {e}"))
    }

    pub fn validate(&self) -> Result<(), String> {
        let interval = self.interval_duration()?;
        if interval.is_zero() {
            return Err("interval: must be > 0".into());
        }
        if self.max_posts == 0 {
            return Err("max_posts must be > 0".into());
        }
        if self.min_posts < 2 {
            return Err("min_posts must be >= 2 (objectiveai cannot score fewer than 2 inputs)".into());
        }
        if self.min_posts > self.max_posts {
            return Err("min_posts must be <= max_posts".into());
        }

        if let Some(qs) = &self.queries {
            for (i, q) in qs.iter().enumerate() {
                q.validate().map_err(|e| format!("queries[{i}]: {e}"))?;
            }
        }

        // A psyop must have at least one input source — either a
        // non-empty `queries` list, or `for_you` configured.
        let has_queries = self.queries.as_ref().is_some_and(|qs| !qs.is_empty());
        let has_for_you = self.for_you.is_some();
        if !has_queries && !has_for_you {
            return Err("psyop must have at least one query or for_you".into());
        }

        if let Some(fy) = &self.for_you {
            fy.validate().map_err(|e| format!("for_you: {e}"))?;
        }
        self.sort.validate().map_err(|e| format!("sort: {e}"))?;

        // Stages may be omitted entirely; if present, each must
        // validate. No-stages runs flow through with max-score
        // survivors at runtime.
        if let Some(stages) = &self.stages {
            for (i, s) in stages.iter().enumerate() {
                s.validate().map_err(|e| format!("stages[{i}]: {e}"))?;
            }
        }

        Ok(())
    }
}
