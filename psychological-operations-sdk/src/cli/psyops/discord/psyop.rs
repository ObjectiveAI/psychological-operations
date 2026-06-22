use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::sort_by::SortBy;
use super::stage::Stage;

/// Psyop family discriminator for the Discord family. Serializes /
/// deserializes as the static string `"discord"`, letting the untagged
/// [`Psyop`](crate::cli::psyops::Psyop) enum tell families apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PsyopType {
    #[default]
    Discord,
}

/// A Discord psyop scores messages pulled from Discord. Ingestion sources are
/// not modeled yet — for now this carries only the scoring/delivery shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PsyOp {
    /// Psyop family tag — always `"discord"`. Defaults to `Discord` when
    /// absent so psyops stored before the tag existed still deserialize.
    #[serde(rename = "type", default)]
    pub psyop_type: PsyopType,

    /// Minimum wall-clock time between runs, as a humantime duration
    /// string (e.g. `"1h 30m"`). Validated at publish time via
    /// [`humantime::parse_duration`]; must be > 0.
    pub interval: String,

    /// Tiebreak ordering applied across the deduped candidate union.
    pub sort: SortBy,

    /// Multi-stage scoring pipeline. `None` or `Some(empty)` means no
    /// scoring — every survivor gets a max score (1.0) and flows through to
    /// delivery as-is.
    #[serde(default, skip_serializing_if = "skip_stages")]
    pub stages: Option<Vec<Stage>>,

    /// Agent tags to deliver survivors to. Empty (the default) means
    /// score-only — no delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_tags: Vec<String>,

    /// Human-facing note delivered to agents alongside this psyop's queued
    /// messages.
    pub message: String,
}

/// Skip-serializing predicate for `stages`: omit the field when it's `None`
/// OR `Some(empty)`. Both shapes mean "no scoring".
fn skip_stages(s: &Option<Vec<Stage>>) -> bool {
    match s {
        None => true,
        Some(v) => v.is_empty(),
    }
}

impl PsyOp {
    /// Parsed form of [`interval`](Self::interval). `Err` carries the same
    /// message `validate()` rejects with.
    pub fn interval_duration(&self) -> Result<std::time::Duration, String> {
        humantime::parse_duration(&self.interval)
            .map_err(|e| format!("interval: invalid humantime duration: {e}"))
    }

    /// Publish-time consistency check.
    pub fn validate(&self) -> Result<(), String> {
        let interval = self.interval_duration()?;
        if interval.is_zero() {
            return Err("interval: must be > 0".into());
        }
        self.sort.validate().map_err(|e| format!("sort: {e}"))?;
        if let Some(stages) = &self.stages {
            for (i, s) in stages.iter().enumerate() {
                s.validate().map_err(|e| format!("stages[{i}]: {e}"))?;
            }
        }
        Ok(())
    }
}
