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
    /// Personalized "For You" timeline input.
    pub for_you: ForYou,

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

    /// When `Some(true)`, every X v2 HTTP call this psyop's run
    /// would otherwise make short-circuits to the in-process
    /// deterministic mock at `psychological_operations_sdk::x::mock` — zero outbound
    /// network traffic to X. objectiveai function / profile calls
    /// are unaffected (they still hit the real network). Absent /
    /// `Some(false)` → real X. Replaces the older
    /// `PSYCHOLOGICAL_OPERATIONS_MOCK_X_API` process-wide env var.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<bool>,

    /// Multi-stage scoring pipeline. Posts are scored by `stages[0]`,
    /// optionally narrowed via the stage's `output_threshold` /
    /// `output_top`, then fed to `stages[1]`, and so on. Must be
    /// non-empty (validated at publish time).
    pub stages: Vec<Stage>,
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

impl PsyOp {
    /// Whether this psyop runs against the in-process X mock instead
    /// of the real X API. Absent / `Some(false)` → real X.
    pub fn mock_enabled(&self) -> bool {
        self.mock.unwrap_or(false)
    }

    /// Publish-time consistency check. Returns a free-form error
    /// string; CLI callers wrap with their own `Error::InvalidPsyop`
    /// variant.
    pub fn validate(&self) -> Result<(), String> {
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
        self.for_you.validate().map_err(|e| format!("for_you: {e}"))?;
        self.sort.validate().map_err(|e| format!("sort: {e}"))?;

        if self.stages.is_empty() {
            return Err("stages must not be empty".into());
        }
        for (i, s) in self.stages.iter().enumerate() {
            s.validate().map_err(|e| format!("stages[{i}]: {e}"))?;
        }

        Ok(())
    }
}
