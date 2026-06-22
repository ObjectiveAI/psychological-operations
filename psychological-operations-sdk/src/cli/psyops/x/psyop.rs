use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::for_you::ForYou;
use super::mentions::Mentions;
use super::query::Query;
use super::sort_by::SortBy;
use super::stage::Stage;
use super::timeline::Timeline;
use super::trigger::Trigger;

/// Psyop family discriminator. Today the only family is X (tweets);
/// serializes / deserializes as the static string `"x"`. Lets the untagged
/// [`PsyOp`](crate::cli::psyops::PsyOp) enum tell families apart as more are
/// added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PsyopType {
    #[default]
    X,
}

/// A psyop scores tweets pulled from one or more X v2 sources.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PsyOp {
    /// Psyop family tag — always `"x"` for now. Defaults to `X` when absent
    /// so psyops stored before the tag existed still deserialize.
    #[serde(rename = "type", default)]
    pub psyop_type: PsyopType,
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
    /// step, and the `fetch_when_for_you_queued` policy entirely; both
    /// round-trip out as "field absent" via `skip_for_you`. Publish-time
    /// validation requires at least one of `queries` (non-empty) or
    /// `for_you` (non-empty) to be present.
    #[serde(default, skip_serializing_if = "skip_for_you")]
    pub for_you: Option<Vec<ForYou>>,

    /// What causes this psyop to run — `manual` or `interval` (a humantime
    /// cadence). See [`Trigger`].
    pub trigger: Trigger,

    /// Tiebreak ordering applied across the deduped candidate union.
    /// Combines with per-source `priority` (priority is primary,
    /// descending; `None` ranks below every `Some(_)`. `sort` is the
    /// tiebreak among equal-priority items).
    pub sort: SortBy,

    /// When `false`, the X-API fetch sources — `queries`, `timeline`, and
    /// `mentions` — are all skipped on a run as long as `for_you` produced
    /// candidates, the rationale being that if the algorithmic feed is
    /// feeding us enough material, paying for those API calls is wasteful.
    /// When `true`, they always run regardless of for-you state. Defaults
    /// to `true` (no implicit skipping).
    #[serde(default = "default_true")]
    pub fetch_when_for_you_queued: bool,

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

    /// Human-facing note delivered to agents alongside this psyop's queued
    /// tweets. Read by the `read_queue` MCP tool (by psyop name, at read
    /// time — not stored on the queue rows) and shown on each psyop-run
    /// group so the agent knows what the run is for / how to engage.
    pub message: String,
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
    pub fn validate(&self) -> Result<(), String> {
        self.trigger.validate().map_err(|e| format!("trigger: {e}"))?;

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
