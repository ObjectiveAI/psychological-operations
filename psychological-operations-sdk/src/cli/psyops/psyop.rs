use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::for_you::ForYou;
use super::mentions::Mentions;
use super::query::Query;
use super::sort_by::SortBy;
use super::stage::Stage;
use super::timeline::Timeline;

/// A psyop scores tweets pulled from one or more X v2 sources.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PsyOp {
    /// Live X v2 search-query inputs. `None` means no query-driven
    /// ingestion for this psyop. An empty `Some(vec![])` is equivalent
    /// to `None` for ingestion purposes; both round-trip out as
    /// "field absent" via `skip_queries`.
    #[serde(default, skip_serializing_if = "skip_queries")]
    pub queries: Option<Vec<Query>>,
    /// Home-timeline (reverse-chronological) inputs, one per agent. Each
    /// paginates the agent's followed-accounts feed up to `max_posts`.
    /// `None`/empty ⇒ no timeline ingestion (round-trips absent).
    #[serde(default, skip_serializing_if = "skip_timeline")]
    pub timeline: Option<Vec<Timeline>>,
    /// Mentions inputs, one per agent. Each paginates the agent's mentions
    /// feed up to `max_posts`. `None`/empty ⇒ no mentions ingestion.
    #[serde(default, skip_serializing_if = "skip_mentions")]
    pub mentions: Option<Vec<Mentions>>,
    /// Personalized "For You" timeline inputs, one per collecting agent.
    /// `None` or `Some(empty)` means no for-you ingestion — the CLI
    /// runtime skips collection, the queue check, the `hydrate_for_you`
    /// step, and the `query_when_for_you_queued` policy entirely; both
    /// round-trip out as "field absent" via `skip_for_you`. Publish-time
    /// validation requires at least one of `queries` (non-empty) or
    /// `for_you` (non-empty) to be present.
    #[serde(default, skip_serializing_if = "skip_for_you")]
    pub for_you: Option<Vec<ForYou>>,

    /// Minimum wall-clock time between runs, as a humantime duration
    /// string (e.g. `"1h 30m"`). `psyops run` records each psyop's
    /// last successful run and skips the psyop (exit 0, with a
    /// warning event) until the interval has elapsed. Validated at
    /// publish time via [`humantime::parse_duration`]; must be > 0.
    pub interval: String,

    /// Hard cap on candidates sent to the scoring function. After the
    /// candidate union is ordered by `(priority, sort/interweave)` it is
    /// truncated to `max_posts` before scoring. Must be > 0.
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

    /// Agent tags to deliver survivors to. After scoring, each survivor
    /// is written to every listed agent's queue and the agent is notified
    /// of its new pending count. Empty (the default) means score-only —
    /// no delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_tags: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Skip-serializing predicate for `queries`: omit the field when
/// it's `None` OR `Some(empty)`. Both shapes mean "no query-driven
/// ingestion" so emitting `"queries": []` would just be noise.
fn skip_queries(q: &Option<Vec<Query>>) -> bool {
    match q {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Skip-serializing predicate for `for_you`: omit the field when
/// it's `None` OR `Some(empty)`. Both shapes mean "no for-you
/// ingestion".
fn skip_for_you(f: &Option<Vec<ForYou>>) -> bool {
    match f {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Skip-serializing predicate for `timeline`: omit when `None`/empty.
fn skip_timeline(t: &Option<Vec<Timeline>>) -> bool {
    match t {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Skip-serializing predicate for `mentions`: omit when `None`/empty.
fn skip_mentions(m: &Option<Vec<Mentions>>) -> bool {
    match m {
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

        if let Some(qs) = &self.queries {
            for (i, q) in qs.iter().enumerate() {
                q.validate().map_err(|e| format!("queries[{i}]: {e}"))?;
            }
        }
        if let Some(ts) = &self.timeline {
            for (i, t) in ts.iter().enumerate() {
                t.validate().map_err(|e| format!("timeline[{i}]: {e}"))?;
            }
        }
        if let Some(ms) = &self.mentions {
            for (i, m) in ms.iter().enumerate() {
                m.validate().map_err(|e| format!("mentions[{i}]: {e}"))?;
            }
        }

        // A psyop must have at least one input source.
        let has_queries = self.queries.as_ref().is_some_and(|qs| !qs.is_empty());
        let has_timeline = self.timeline.as_ref().is_some_and(|ts| !ts.is_empty());
        let has_mentions = self.mentions.as_ref().is_some_and(|ms| !ms.is_empty());
        let has_for_you = self.for_you.as_ref().is_some_and(|fy| !fy.is_empty());
        if !has_queries && !has_timeline && !has_mentions && !has_for_you {
            return Err("psyop must have at least one query, timeline, mentions, or for_you".into());
        }

        if let Some(fys) = &self.for_you {
            for (i, fy) in fys.iter().enumerate() {
                fy.validate().map_err(|e| format!("for_you[{i}]: {e}"))?;
            }
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
