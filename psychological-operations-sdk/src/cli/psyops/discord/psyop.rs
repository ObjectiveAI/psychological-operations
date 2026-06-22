use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::all_dms::AllDms;
use super::channel::Channel;
use super::dm::Dm;
use super::server::Server;
use super::sort_by::SortBy;
use super::stage::Stage;
use super::trigger::Trigger;

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

    /// Channel-read sources — paginate specific Discord channels' histories,
    /// one entry per channel. `None`/empty ⇒ no channel ingestion.
    #[serde(default, skip_serializing_if = "skip_channels")]
    pub channels: Option<Vec<Channel>>,

    /// DM-read sources — paginate the bot's DM history with specific users,
    /// one entry per user. `None`/empty ⇒ no per-user DM ingestion.
    #[serde(default, skip_serializing_if = "skip_dms")]
    pub dms: Option<Vec<Dm>>,

    /// All-DMs source — paginate across every DM channel the bot is part of.
    /// `None` ⇒ no all-DMs ingestion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_dms: Option<AllDms>,

    /// Server-read sources — paginate across a whole server's channels, one
    /// entry per server. `None`/empty ⇒ no server ingestion.
    #[serde(default, skip_serializing_if = "skip_servers")]
    pub servers: Option<Vec<Server>>,

    /// What causes this psyop to run — `manual` or `interval` (a humantime
    /// cadence). See [`Trigger`].
    pub trigger: Trigger,

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

/// Skip-serializing predicate for `channels`: omit when `None`/empty.
fn skip_channels(c: &Option<Vec<Channel>>) -> bool {
    match c {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Skip-serializing predicate for `dms`: omit when `None`/empty.
fn skip_dms(d: &Option<Vec<Dm>>) -> bool {
    match d {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Skip-serializing predicate for `servers`: omit when `None`/empty.
fn skip_servers(s: &Option<Vec<Server>>) -> bool {
    match s {
        None => true,
        Some(v) => v.is_empty(),
    }
}

impl PsyOp {
    /// Publish-time consistency check.
    pub fn validate(&self) -> Result<(), String> {
        self.trigger.validate().map_err(|e| format!("trigger: {e}"))?;

        if let Some(cs) = &self.channels {
            for (i, c) in cs.iter().enumerate() {
                c.validate().map_err(|e| format!("channels[{i}]: {e}"))?;
            }
        }
        if let Some(ds) = &self.dms {
            for (i, d) in ds.iter().enumerate() {
                d.validate().map_err(|e| format!("dms[{i}]: {e}"))?;
            }
        }
        if let Some(a) = &self.all_dms {
            a.validate().map_err(|e| format!("all_dms: {e}"))?;
        }
        if let Some(ss) = &self.servers {
            for (i, s) in ss.iter().enumerate() {
                s.validate().map_err(|e| format!("servers[{i}]: {e}"))?;
            }
        }

        // A psyop must have at least one input source.
        let has_channels = self.channels.as_ref().is_some_and(|cs| !cs.is_empty());
        let has_dms = self.dms.as_ref().is_some_and(|ds| !ds.is_empty());
        let has_all_dms = self.all_dms.is_some();
        let has_servers = self.servers.as_ref().is_some_and(|s| !s.is_empty());
        if !has_channels && !has_dms && !has_all_dms && !has_servers {
            return Err("psyop must have at least one channel, dm, all_dms, or server".into());
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
