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
    #[envconfig(from = "PSYCHOLOGICAL_OPERATIONS_BASE_DIR")]
    base_dir: Option<String>,
}

impl EnvConfigBuilder {
    pub fn build(self) -> ConfigBuilder {
        ConfigBuilder {
            base_dir: self.base_dir,
        }
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    pub base_dir: Option<String>,
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
        Config { base_dir: self.base_dir }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub base_dir: Option<String>,
}

impl Config {
    /// Resolve the on-disk base directory. Explicit env override
    /// (`PSYCHOLOGICAL_OPERATIONS_BASE_DIR`) wins; otherwise
    /// `~/.psychological-operations`.
    pub fn base_dir(&self) -> PathBuf {
        if let Some(d) = &self.base_dir {
            return PathBuf::from(d);
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
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
    /// Chrome native-messaging host. Reads framed JSON on stdin
    /// (from psychological-operations-chrome-extension) and writes captured tweets into
    /// the local DB. Identity (psyop + commit) is resolved from the
    /// PSYOP_NAME / PSYOP_COMMIT_SHA env vars set by the launcher
    /// when Chrome was spawned with this profile.
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
