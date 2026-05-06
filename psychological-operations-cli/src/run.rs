use clap::{Parser, Subcommand};

use crate::chrome;
use crate::ingest;
use crate::invent;
use crate::notifications;
use crate::psyops;

#[derive(Parser)]
#[command(name = "psychological-operations")]
#[command(about = "ObjectiveAI-driven X scoring pipeline")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage psyops (list/get/enable/disable/publish/run/notifications)
    Psyops {
        #[command(subcommand)]
        command: psyops::Commands,
    },
    /// Global notification destinations
    Notifications {
        #[command(subcommand)]
        command: notifications::Commands,
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
    /// Launch the embedded Chromium for a psyop. Materializes the
    /// embedded Chrome bundle on first run, sets up the native-
    /// messaging host registration, and spawns Chromium with a
    /// per-psyop profile and PSYOP_NAME / PSYOP_COMMIT_SHA env.
    Browse {
        /// Psyop name. Used for the per-psyop profile directory.
        #[arg(long)]
        psyop: String,
        /// Optional explicit commit SHA. Defaults to git HEAD inside
        /// <psyops_dir>/<psyop>/.
        #[arg(long)]
        commit: Option<String>,
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

pub async fn run<I, T>(args: I) -> Result<String, String>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|e| e.to_string())?;
    let output = match cli.command {
        Commands::Psyops { command } => command.handle().await,
        Commands::Notifications { command } => command.handle(),
        Commands::Invent { command } => command.handle(),
        Commands::NativeHost => ingest::run().await,
        Commands::Browse { psyop, commit } => chrome::browse(psyop, commit).await,
    }
    .map_err(|e| e.to_string())?;
    Ok(output.to_string())
}
