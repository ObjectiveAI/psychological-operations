//! CLI command surface.
//!
//! All clap-driven `Commands` enums + their `handle` dispatch live
//! under this module. The matching root-level modules
//! (`crate::psyops`, ...) hold the type definitions and business
//! logic the handlers call into.
//!
//! The top-level entry point is [`run`] — invoked from `main.rs`
//! with the raw argv. It parses via clap, dispatches to the
//! appropriate per-domain `Commands::handle`, and stringifies the
//! resulting `Output`.

use clap::{Parser, Subcommand};

use crate::context::Context;

pub mod agents;
pub mod daemon;
pub mod mcp;
pub mod psyops;
pub mod twitch_app;
pub mod x_app;

#[derive(Parser)]
#[command(name = "psychological-operations")]
#[command(about = "ObjectiveAI-driven X scoring pipeline")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage psyops (list/get/enable/disable/publish/run/browse/oauth)
    Psyops {
        #[command(subcommand)]
        command: psyops::Commands,
    },
    /// Manage agents (oauth)
    Agents {
        #[command(subcommand)]
        command: agents::Commands,
    },
    /// Master X dev-account / X-App credentials setup.
    #[command(name = "x-app")]
    XApp {
        #[command(subcommand)]
        command: x_app::Commands,
    },
    /// Master Twitch application credentials setup.
    #[command(name = "twitch-app")]
    TwitchApp {
        #[command(subcommand)]
        command: twitch_app::Commands,
    },
    /// Embedded X-API MCP server: begin (or attach to) a per-agent
    /// supervised instance.
    Mcp {
        #[command(subcommand)]
        command: mcp::Commands,
    },
    /// Resident Discord gateway daemon: `begin` runs it (never returns).
    /// Launched by the objectiveai daemon for this plugin.
    Daemon {
        #[command(subcommand)]
        command: daemon::Commands,
    },
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

pub async fn run<I, T>(args: I, ctx: &Context) -> bool
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // Clap returns Err for `--help`, `--version`, and "no subcommand
        // given" because none has a `Cli` value to dispatch on. All three
        // are informational, not failures — emit as a typed `Output::Help`
        // line, then succeed (exit 0).
        Err(e) if is_informational(&e) => {
            crate::output::emit_output(psychological_operations_sdk::cli::Output::Help(
                psychological_operations_sdk::cli::output::Help {
                    text: e.to_string(),
                },
            ));
            return true;
        }
        Err(e) => {
            crate::output::emit_fatal(e);
            return false;
        }
    };
    match cli.command {
        Commands::Psyops { command } => command.handle(ctx).await,
        Commands::Agents { command } => command.handle(ctx).await,
        Commands::XApp { command } => command.handle(ctx).await,
        Commands::TwitchApp { command } => command.handle(ctx).await,
        Commands::Mcp { command } => command.handle(ctx).await,
        Commands::Daemon { command } => command.handle(ctx).await,
    }
}
