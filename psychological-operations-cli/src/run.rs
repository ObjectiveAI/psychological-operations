use std::path::PathBuf;

use envconfig::Envconfig;

// ---------------------------------------------------------------------------
// Env-driven runtime config (3-struct pattern; mirrors objectiveai-cli)
// ---------------------------------------------------------------------------

#[derive(Envconfig)]
struct EnvConfigBuilder {
    /// Root of the remaining filesystem state — the CEF/Chromium
    /// profile tree + browser cache + host-protocol exec-tmp live under
    /// it. Everything else is in postgres now. Required; unwrapped at
    /// `build()` (we panic if absent).
    #[envconfig(from = "OBJECTIVEAI_STATE_DIR")]
    state_dir: Option<String>,
    /// Postgres connection URL — the single persistence layer.
    /// Required; unwrapped at `build()`.
    #[envconfig(from = "OBJECTIVEAI_POSTGRES_URL")]
    postgres_url: Option<String>,
    #[envconfig(from = "OBJECTIVEAI_AGENT_ID")]
    objectiveai_agent_id: Option<String>,
    #[envconfig(from = "OBJECTIVEAI_AGENT_FULL_ID")]
    objectiveai_agent_full_id: Option<String>,
    #[envconfig(from = "OBJECTIVEAI_AGENT_REMOTE")]
    objectiveai_agent_remote: Option<String>,
    #[envconfig(from = "OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY")]
    objectiveai_agent_instance_hierarchy: Option<String>,
}

impl EnvConfigBuilder {
    pub fn build(self) -> ConfigBuilder {
        ConfigBuilder {
            state_dir: self.state_dir,
            postgres_url: self.postgres_url,
            objectiveai_agent_id: self.objectiveai_agent_id,
            objectiveai_agent_full_id: self.objectiveai_agent_full_id,
            objectiveai_agent_remote: self.objectiveai_agent_remote,
            objectiveai_agent_instance_hierarchy: self.objectiveai_agent_instance_hierarchy,
        }
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    pub state_dir: Option<String>,
    pub postgres_url: Option<String>,
    pub objectiveai_agent_id: Option<String>,
    pub objectiveai_agent_full_id: Option<String>,
    pub objectiveai_agent_remote: Option<String>,
    pub objectiveai_agent_instance_hierarchy: Option<String>,
}

impl Envconfig for ConfigBuilder {
    #[allow(deprecated)]
    fn init() -> Result<Self, envconfig::Error> {
        EnvConfigBuilder::init().map(|e| e.build())
    }

    fn init_from_env() -> Result<Self, envconfig::Error> {
        EnvConfigBuilder::init_from_env().map(|e| e.build())
    }

    fn init_from_hashmap(
        h: &std::collections::HashMap<String, String>,
    ) -> Result<Self, envconfig::Error> {
        EnvConfigBuilder::init_from_hashmap(h).map(|e| e.build())
    }
}

impl ConfigBuilder {
    pub fn build(self) -> Config {
        Config {
            // Required — unwrapped here, after env init. Absence is a
            // hard misconfiguration: panic with a clear message.
            state_dir: PathBuf::from(self.state_dir.expect(
                "OBJECTIVEAI_STATE_DIR must be set (the state root)",
            )),
            postgres_url: self.postgres_url.expect(
                "OBJECTIVEAI_POSTGRES_URL must be set",
            ),
            objectiveai_agent_id: self.objectiveai_agent_id,
            objectiveai_agent_full_id: self.objectiveai_agent_full_id,
            objectiveai_agent_remote: self.objectiveai_agent_remote,
            objectiveai_agent_instance_hierarchy: self
                .objectiveai_agent_instance_hierarchy
                .unwrap_or_else(|| "psychological-operations".to_string()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Root of the remaining filesystem state (env
    /// `OBJECTIVEAI_STATE_DIR`). Only the CEF/Chromium profile tree, the
    /// extracted browser bundle cache, and the host-protocol `exec-tmp`
    /// live under it now — all other state moved to postgres. Assumed to
    /// already exist. Required (panics if unset).
    pub state_dir: PathBuf,
    /// Postgres connection URL (env `OBJECTIVEAI_POSTGRES_URL`) — the
    /// single persistence layer. Required.
    pub postgres_url: String,
    /// Default agent id (env `OBJECTIVEAI_AGENT_ID`). Currently unused —
    /// captured for parity with the objectiveai agent-environment
    /// contract (alongside `objectiveai_agent_full_id` / `_remote` /
    /// `_instance_hierarchy`).
    pub objectiveai_agent_id: Option<String>,
    /// Agent's fully-qualified id (env `OBJECTIVEAI_AGENT_FULL_ID`).
    /// Currently unused — captured for parity with the objectiveai
    /// agent-environment contract.
    pub objectiveai_agent_full_id: Option<String>,
    /// Agent's remote ref (env `OBJECTIVEAI_AGENT_REMOTE`).
    /// Currently unused — captured for parity with the objectiveai
    /// agent-environment contract.
    pub objectiveai_agent_remote: Option<String>,
    /// Agent instance hierarchy (env
    /// `OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY`). Required; defaults to
    /// `"psychological-operations"` when the env var is unset.
    /// Currently unused.
    pub objectiveai_agent_instance_hierarchy: String,
}

impl Config {
    /// The state root (env `OBJECTIVEAI_STATE_DIR`). All state files
    /// live directly under it; assumed to already exist.
    pub fn state_dir(&self) -> PathBuf {
        self.state_dir.clone()
    }
}

/// Build the runtime config from the process environment.
pub fn load_config() -> Config {
    ConfigBuilder::init_from_env().unwrap_or_default().build()
}

// Output type lives in the SDK now —
// `psychological_operations_sdk::cli::Output`. Call sites
// import it directly; lib.rs intentionally does not re-export.

