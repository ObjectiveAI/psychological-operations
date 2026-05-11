use std::path::PathBuf;

use clap::{Parser, Subcommand};
use envconfig::Envconfig;

use crate::x_app;
use crate::ingest;
use crate::invent;
use crate::targets;
use crate::psyops;

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
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_MOCK_X_API")]
    mock_x_api: Option<String>,
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_NAME")]
    commit_author_name: Option<String>,
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_EMAIL")]
    commit_author_email: Option<String>,
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_COMMIT_TIME")]
    commit_time: Option<String>,
}

impl EnvConfigBuilder {
    pub fn build(self) -> ConfigBuilder {
        fn parse_bool(s: &str) -> bool {
            let v = s.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        ConfigBuilder {
            objectiveai_base_dir: self.objectiveai_base_dir,
            mock_x_api:           self.mock_x_api.map(|s| parse_bool(&s)),
            commit_author_name:   self.commit_author_name,
            commit_author_email:  self.commit_author_email,
            commit_time:          self.commit_time
                .and_then(|s| s.trim().parse::<i64>().ok()),
        }
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    pub objectiveai_base_dir: Option<String>,
    pub mock_x_api:           Option<bool>,
    pub commit_author_name:   Option<String>,
    pub commit_author_email:  Option<String>,
    pub commit_time:          Option<i64>,
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
            objectiveai_base_dir: self.objectiveai_base_dir,
            mock_x_api:           self.mock_x_api.unwrap_or(false),
            commit_author_name:   self.commit_author_name,
            commit_author_email:  self.commit_author_email,
            commit_time:          self.commit_time,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// objectiveai-cli's base directory (shared env: `CONFIG_BASE_DIR`).
    /// When `None`, defaults to `~/.objectiveai`. Our state goes in
    /// `<this>/plugins/.psychological-operations/`.
    pub objectiveai_base_dir: Option<String>,
    /// When true, every X HTTP call short-circuits to a
    /// deterministic mock keyed on the input. Set via
    /// `PSYCHOLOGICAL_OPERATIONS_MOCK_X_API`.
    pub mock_x_api:         bool,
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
    /// `<objectiveai_base>/plugins/.psychological-operations`.
    ///
    /// Leading `.` so the objectiveai-cli plugin dispatcher (which
    /// looks for bare files in `plugins/`) doesn't try to invoke
    /// this dir as a plugin binary.
    pub fn base_dir(&self) -> PathBuf {
        self.objectiveai_base_dir()
            .join("plugins")
            .join(".psychological-operations")
    }
}

/// Build the runtime config from the process environment.
pub fn load_config() -> Config {
    ConfigBuilder::init_from_env().unwrap_or_default().build()
}

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "psychological-operations")]
#[command(about = "ObjectiveAI-driven X scoring pipeline")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage psyops (list/get/enable/disable/publish/run/browse/oauth/targets)
    Psyops {
        #[command(subcommand)]
        command: psyops::Commands,
    },
    /// Global target destinations
    Targets {
        #[command(subcommand)]
        command: targets::Commands,
    },
    /// Invent a function for scoring posts
    Invent {
        #[command(subcommand)]
        command: invent::Commands,
    },
    /// Chromium native-messaging host. Reads framed JSON on stdin
    /// (from psychological-operations-chromium-extension) and writes captured tweets into
    /// the local DB. Identity (psyop + commit) is resolved from the
    /// PSYOP_NAME / PSYOP_COMMIT_SHA env vars set by the launcher
    /// when Chromium was spawned with this profile.
    NativeHost,
    /// Master X dev-account / X-App credentials setup.
    #[command(name = "x_app")]
    XApp {
        #[command(subcommand)]
        command: x_app::Commands,
    },
}

pub enum Output {
    ConfigGet(String),
    ConfigSet,
    Api(String),
    Empty,
}

impl std::fmt::Display for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Output::ConfigGet(s) => write!(f, "{s}"),
            Output::ConfigSet => write!(f, "ok"),
            Output::Api(s) => write!(f, "{s}"),
            Output::Empty => Ok(()),
        }
    }
}

pub async fn run<I, T>(args: I, cfg: &Config) -> Result<String, String>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|e| e.to_string())?;
    let output = match cli.command {
        Commands::Psyops { command } => command.handle(cfg).await,
        Commands::Targets { command } => command.handle(cfg).await,
        Commands::Invent { command } => command.handle(cfg),
        Commands::NativeHost => ingest::run(cfg).await,
        Commands::XApp { command } => command.handle(cfg).await,
    }
    .map_err(|e| e.to_string())?;
    Ok(output.to_string())
}
