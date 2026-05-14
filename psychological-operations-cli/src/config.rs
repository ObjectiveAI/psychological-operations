use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::run::Config as RuntimeConfig;
use crate::targets::destinations::{stdout, x, Destination};

// ---------------------------------------------------------------------------
// Paths — all rooted at `cfg.base_dir()` so an env-var override
// (PSYCHOLOGICAL_OPERATIONS_BASE_DIR) re-homes everything.
// ---------------------------------------------------------------------------

pub fn config_path(cfg: &RuntimeConfig) -> PathBuf {
    cfg.base_dir().join("config.json")
}

pub fn psyops_dir(cfg: &RuntimeConfig) -> PathBuf {
    cfg.base_dir().join("psyops")
}

pub fn db_path(cfg: &RuntimeConfig) -> PathBuf {
    cfg.base_dir().join("data.db")
}

// ---------------------------------------------------------------------------
// Per-name overrides
// ---------------------------------------------------------------------------

/// Target destinations + flags that apply to one psyop. Used both as
/// the `base` of a `PsyopOverrides` and as the value of each commit-specific
/// entry under `commits`.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct PsyopConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Destination>,
    /// `Some(true)`  → disabled, `Some(false)` → forced enabled,
    /// `None`        → inherit from the next layer (base, then default
    /// behaviour, which is enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

impl PsyopConfig {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty() && self.disabled.is_none()
    }
}

/// Two-level overrides for a psyop: a `base` that applies to every commit
/// of that psyop, plus an optional `commits` map keyed by SHA. When
/// resolving a value for a specific commit, the commit-level entry shadows
/// `base`. For `targets`, the rule is replace-or-fall-back (never
/// merged); for scalar fields the commit-level wins only if it's `Some(_)`.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct PsyopOverrides {
    #[serde(default, skip_serializing_if = "PsyopConfig::is_empty")]
    pub base: PsyopConfig,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commits: BTreeMap<String, PsyopConfig>,
}

impl PsyopOverrides {
    pub fn is_empty(&self) -> bool {
        self.base.is_empty() && self.commits.is_empty()
    }

    /// Targets that apply at `commit_sha`. If a commit-level entry
    /// exists with non-empty targets, those are used exclusively;
    /// otherwise the base targets are used. Never a concatenation.
    pub fn targets_for(&self, commit_sha: &str) -> &[Destination] {
        if let Some(c) = self.commits.get(commit_sha) {
            if !c.targets.is_empty() {
                return &c.targets;
            }
        }
        &self.base.targets
    }

    pub fn disabled_for(&self, commit_sha: &str) -> bool {
        if let Some(c) = self.commits.get(commit_sha) {
            if let Some(d) = c.disabled { return d; }
        }
        self.base.disabled.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Config root
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    /// Global target destinations — fire on every psyop run.
    #[serde(default)]
    pub targets: Vec<Destination>,
    /// Per-psyop overrides keyed by psyop name. Each entry has a `base`
    /// applied to every commit, plus an optional `commits` map that can
    /// shadow base values for a specific SHA.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub psyops: BTreeMap<String, PsyopOverrides>,
}

pub fn load(cfg: &RuntimeConfig) -> Config {
    let path = config_path(cfg);
    if !path.exists() {
        // First-run users get a useful out-of-the-box behavior:
        // every survivor of scoring gets liked on X. Users who
        // don't want it can `targets del 0` to drop it (which
        // creates an explicit `{"targets":[]}` on disk; a
        // file-present empty list is honored as-is, so the opt-out
        // sticks).
        return default_initial_config();
    }
    let data = std::fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str(&data).unwrap_or_default()
}

fn default_initial_config() -> Config {
    Config {
        targets: vec![
            Destination::X(x::X { r#type: x::XType::Like }),
            // Stdout emits one `Event::TargetDelivered` per drained
            // delivery with the full `json_body`; consumers see the
            // ranked survivors structured as JSON in the PluginOutput
            // stream.
            Destination::Stdout(stdout::Stdout::default()),
        ],
        psyops:  BTreeMap::new(),
    }
}

pub fn save(config: &Config, cfg: &RuntimeConfig) -> Result<(), crate::error::Error> {
    let path = config_path(cfg);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, json + "\n")?;
    Ok(())
}
