use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What causes this psyop to run. Tagged on `"type"`:
///
/// - **`manual`** — only runs when explicitly invoked.
/// - **`interval`** — runs on a fixed cadence; `interval` is a humantime
///   duration string (e.g. `"1h 30m"`), must be > 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Interval { interval: String },
}

impl Trigger {
    /// Publish-time check: an `interval` trigger must carry a valid,
    /// non-zero humantime duration.
    pub fn validate(&self) -> Result<(), String> {
        if let Trigger::Interval { interval } = self {
            let d = humantime::parse_duration(interval)
                .map_err(|e| format!("interval: invalid humantime duration: {e}"))?;
            if d.is_zero() {
                return Err("interval: must be > 0".into());
            }
        }
        Ok(())
    }
}
