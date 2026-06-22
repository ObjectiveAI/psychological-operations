//! Psyop type graph. Each psyop *family* lives in its own submodule: [`x`]
//! (tweets) and [`discord`] (Discord messages). The top-level [`PsyOp`] enum
//! is the published shape — untagged, discriminated by each family body's
//! `type` field ([`x::PsyopType`] / [`discord::PsyopType`]).
//!
//! Pure data + `validate()` methods. Runtime concerns (db-backed load/save,
//! Python evaluation against live rows) live in the CLI alongside the
//! scoring pipeline.

pub mod discord;
pub mod x;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A published psyop. Untagged: each family body carries a `type`
/// discriminator (`"x"` / `"discord"`) so serde can tell them apart. (The
/// family bodies are `x::PsyOp` / `discord::PsyOp`; this is the umbrella.)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PsyOp {
    X(x::PsyOp),
    Discord(discord::PsyOp),
}

impl PsyOp {
    /// Publish-time validation, dispatched to the family.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            PsyOp::X(p) => p.validate(),
            PsyOp::Discord(p) => p.validate(),
        }
    }

    /// The psyop's run cadence: `Ok(None)` for a `manual` trigger (runs only
    /// when explicitly named), `Ok(Some(d))` for an `interval` trigger (the
    /// parsed humantime duration). The two families' `Trigger` enums are
    /// identical-shaped, so this unifies them for the run gate.
    pub fn trigger_interval(&self) -> Result<Option<std::time::Duration>, String> {
        let interval_str = match self {
            PsyOp::X(p) => match &p.trigger {
                x::trigger::Trigger::Manual => return Ok(None),
                x::trigger::Trigger::Interval { interval } => interval,
            },
            PsyOp::Discord(p) => match &p.trigger {
                discord::trigger::Trigger::Manual => return Ok(None),
                discord::trigger::Trigger::Interval { interval } => interval,
            },
        };
        humantime::parse_duration(interval_str)
            .map(Some)
            .map_err(|e| format!("trigger.interval: invalid humantime duration: {e}"))
    }
}
