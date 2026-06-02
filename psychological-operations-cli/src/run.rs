use std::path::PathBuf;

use envconfig::Envconfig;

// ---------------------------------------------------------------------------
// Env-driven runtime config (3-struct pattern; mirrors objectiveai-cli)
// ---------------------------------------------------------------------------

#[derive(Envconfig)]
struct EnvConfigBuilder {
    /// objectiveai's base directory. We share the same env name so a
    /// single setting controls both objectiveai-cli and this plugin.
    /// Default `~/.objectiveai`. Our state goes in
    /// `<base>/plugins/.psychological-operations/`.
    #[envconfig(from = "CONFIG_BASE_DIR")]
    objectiveai_base_dir: Option<String>,
    #[envconfig(from = "OBJECTIVEAI_INSTANCE_HIERARCHY")]
    objectiveai_instance_hierarchy: Option<String>,
    #[envconfig(from = "OBJECTIVEAI_AGENT_ID")]
    objectiveai_agent_id: Option<String>,
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_NAME")]
    commit_author_name: Option<String>,
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_EMAIL")]
    commit_author_email: Option<String>,
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_COMMIT_TIME")]
    commit_time: Option<String>,
}

impl EnvConfigBuilder {
    pub fn build(self) -> ConfigBuilder {
        ConfigBuilder {
            objectiveai_base_dir:           self.objectiveai_base_dir,
            objectiveai_instance_hierarchy: self.objectiveai_instance_hierarchy,
            objectiveai_agent_id:           self.objectiveai_agent_id,
            commit_author_name:             self.commit_author_name,
            commit_author_email:            self.commit_author_email,
            commit_time:                    self.commit_time
                .and_then(|s| s.trim().parse::<i64>().ok()),
        }
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    pub objectiveai_base_dir:           Option<String>,
    pub objectiveai_instance_hierarchy: Option<String>,
    pub objectiveai_agent_id:           Option<String>,
    pub commit_author_name:             Option<String>,
    pub commit_author_email:            Option<String>,
    pub commit_time:                    Option<i64>,
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
            objectiveai_base_dir:           self.objectiveai_base_dir,
            objectiveai_instance_hierarchy: self.objectiveai_instance_hierarchy,
            objectiveai_agent_id:           self.objectiveai_agent_id,
            commit_author_name:             self.commit_author_name,
            commit_author_email:            self.commit_author_email,
            commit_time:                    self.commit_time,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// objectiveai-cli's base directory (shared env: `CONFIG_BASE_DIR`).
    /// When `None`, defaults to `~/.objectiveai`. Our state goes in
    /// `<this>/plugins/.psychological-operations/`.
    pub objectiveai_base_dir: Option<String>,
    /// Caller agent instance hierarchy (env
    /// `OBJECTIVEAI_INSTANCE_HIERARCHY`). The lineage of the
    /// agent invoking this CLI — e.g. `cli/parent/child`. Used by
    /// `agents queue handle` to key the `handler_map` lookup so
    /// different callers each manage their own objectiveai handler
    /// agents against the same shared per-agent queue.
    pub objectiveai_instance_hierarchy: Option<String>,
    /// Default agent id (env `OBJECTIVEAI_AGENT_ID`).
    /// Used as the fallback by `mcp begin --agent` (and any other
    /// command that needs an agent and doesn't get one on the
    /// command line).
    pub objectiveai_agent_id: Option<String>,
    /// Commit author name baked into git commits produced by
    /// `psyops publish`. Default `"psychological-operations"`.
    /// Set via `PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_NAME`.
    pub commit_author_name:  Option<String>,
    /// Commit author email. Default `"psyops@localhost"`.
    /// Set via `PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_EMAIL`.
    pub commit_author_email: Option<String>,
    /// Commit time (epoch seconds). When `Some`, all commits use
    /// this fixed timestamp — yields reproducible commit SHAs
    /// across machines (used by integration tests). When `None`,
    /// each commit uses the current wall clock.
    /// Set via `PSYCHOLOGICAL_OPERATIONS_COMMIT_TIME`.
    pub commit_time:         Option<i64>,
}

impl Config {
    /// objectiveai-cli's base directory. Honors `CONFIG_BASE_DIR`,
    /// falls back to `~/.objectiveai`.
    pub fn objectiveai_base_dir(&self) -> PathBuf {
        if let Some(d) = &self.objectiveai_base_dir {
            return PathBuf::from(d);
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".objectiveai")
    }

    /// Our state directory:
    /// `<objectiveai_base>/plugins/psychological-operations`.
    ///
    /// Matches objectiveai-cli's per-plugin subdir install layout
    /// (`<plugins_dir>/<repository>/`). State files (data.db, psyops/,
    /// config.json, x_app.json, tokens/, chromium profiles) live in
    /// this dir alongside the installed binary.
    pub fn base_dir(&self) -> PathBuf {
        self.objectiveai_base_dir()
            .join("plugins")
            .join("psychological-operations")
    }
}

/// Build the runtime config from the process environment.
pub fn load_config() -> Config {
    ConfigBuilder::init_from_env().unwrap_or_default().build()
}

// ---------------------------------------------------------------------------
// Output type returned by command handlers
// ---------------------------------------------------------------------------

pub enum Output {
    ConfigGet(String),
    ConfigSet,
    Api(String),
    /// X-API MCP `begin` result. The URL of the supervised MCP
    /// (which serves all agents/modes; the caller multiplexes via
    /// `headers` on the initial connect).
    Mcp {
        url: String,
        /// HTTP headers the MCP client must send on its initial
        /// POST. `X-PSYOP-X-API-AGENT` and `X-PSYOP-X-API-MODE`
        /// pin the session's identity + tool surface for the rest
        /// of its lifetime.
        headers: std::collections::BTreeMap<String, String>,
    },
    Empty,
}

impl std::fmt::Display for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Output::ConfigGet(s) => write!(f, "{s}"),
            Output::ConfigSet => write!(f, "ok"),
            Output::Api(s) => write!(f, "{s}"),
            Output::Mcp { url, headers } => {
                write!(f, "{}", serde_json::json!({
                    "type": "mcp",
                    "url": url,
                    "headers": headers,
                }))
            }
            Output::Empty => Ok(()),
        }
    }
}

