use std::path::PathBuf;

use clap::{Parser, Subcommand};
use envconfig::Envconfig;

use crate::agents;
use crate::mcp;
use crate::x_app;
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
            objectiveai_base_dir: self.objectiveai_base_dir,
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
    /// Manage agents (oauth)
    Agents {
        #[command(subcommand)]
        command: agents::Commands,
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
    /// Master X dev-account / X-App credentials setup.
    #[command(name = "x_app")]
    XApp {
        #[command(subcommand)]
        command: x_app::Commands,
    },
    /// Embedded X-API MCP server: begin (or attach to) a per-agent
    /// supervised instance.
    Mcp {
        #[command(subcommand)]
        command: mcp::Commands,
    },
}

pub enum Output {
    ConfigGet(String),
    ConfigSet,
    Api(String),
    /// X-API MCP `begin` result. The URL of the supervised MCP for
    /// the requested agent (newly spawned or re-attached).
    Mcp { url: String },
    Empty,
}

impl std::fmt::Display for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Output::ConfigGet(s) => write!(f, "{s}"),
            Output::ConfigSet => write!(f, "ok"),
            Output::Api(s) => write!(f, "{s}"),
            Output::Mcp { url } => {
                write!(f, "{}", serde_json::json!({"type": "mcp", "url": url}))
            }
            Output::Empty => Ok(()),
        }
    }
}

/// Three clap error kinds carry rendered text the user explicitly
/// asked for (`--help`, `--version`) or that clap auto-renders when
/// invocation lacks a subcommand. They're informational — not parse
/// failures — and should bypass the fatal-Error emission path.
///
/// Mirrors the upstream objectiveai-cli fix (see `deleteme.md`
/// scaffolding doc).
fn is_informational(e: &clap::Error) -> bool {
    use clap::error::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    )
}

pub async fn run<I, T>(args: I, cfg: &Config) -> Result<String, String>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // Clap returns Err for `--help`, `--version`, and "no subcommand
        // given" because none has a `Cli` value to dispatch on. All three
        // are informational, not failures — return them through the Ok
        // arm so main.rs emits via `emit_notification_from_payload`
        // (Notification, exit 0) instead of `emit_error` (Error, exit 1).
        // Matches the structure of objectiveai's own upstream fix in
        // `deleteme.md`.
        Err(e) if is_informational(&e) => return Ok(e.to_string()),
        Err(e) => return Err(e.to_string()),
    };
    let output = match cli.command {
        Commands::Psyops { command } => command.handle(cfg).await,
        Commands::Agents { command } => command.handle(cfg).await,
        Commands::Targets { command } => command.handle(cfg).await,
        Commands::Invent { command } => command.handle(cfg),
        Commands::XApp { command } => command.handle(cfg).await,
        Commands::Mcp { command } => command.handle(cfg).await,
    }
    .map_err(|e| e.to_string())?;
    Ok(output.to_string())
}
