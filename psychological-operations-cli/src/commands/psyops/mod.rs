//! `psyops` subcommand surface.
//!
//! The type defs (`PsyOp`, `PsyopSource`, `PublishArgs`, ...), the
//! run loop, the browse helpers, and the per-command body fns
//! (`list`, `get`, `set_disabled`, `publish`, `delete`) all stay in
//! `crate::psyops`. This file owns the clap surface and the dispatch
//! that calls into them.

use clap::Subcommand;

use crate::error::Error;
use crate::psyops::{self, PublishArgs};

pub mod targets;

#[derive(Subcommand)]
pub enum Commands {
    /// List all psyops on disk. `enabled` reflects the resolved state at
    /// each psyop's current commit. `--enabled` and `--disabled` are
    /// mutually exclusive filters.
    List {
        #[arg(long, conflicts_with = "disabled")]
        enabled: bool,
        #[arg(long)]
        disabled: bool,
    },
    /// Print the on-disk JSON definition of a psyop.
    Get {
        name: String,
    },
    /// Mark a psyop as enabled. With `--commit <sha>` only affects that
    /// commit; otherwise updates the base flag.
    Enable {
        name: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Mark a psyop as disabled. With `--commit <sha>` only affects that
    /// commit; otherwise updates the base flag.
    Disable {
        name: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Publish a psyop definition (writes psyop.json + commits in its repo).
    Publish {
        #[command(flatten)]
        args: PublishArgs,
    },
    /// Delete a psyop. Removes its directory under `<psyops_dir>/<name>/`
    /// (including the git repo) and drops any per-psyop overrides from
    /// `config.json`. Errors if the psyop dir is missing.
    Delete {
        name: String,
    },
    /// Run enabled psyops in rounds: each round runs all psyops that have
    /// enough data concurrently; later rounds pick up psyops whose inputs
    /// depend on earlier rounds' scores. With no flags, runs the full set.
    /// `--name X` narrows the run to one psyop; `--commit Y` additionally
    /// requires the psyop's HEAD to match Y. `--commit` without `--name`
    /// is rejected.
    Run {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, requires = "name")]
        commit: Option<String>,
        /// Pass-through to `objectiveai` for deterministic mock
        /// outputs. Used by integration tests; optional otherwise.
        #[arg(long)]
        seed: Option<i64>,
    },
    /// Open the embedded browser for each psyop in turn so the
    /// operator can scroll x.com and save tweet IDs. Blocks on each
    /// browser's exit before opening the next. With `--name <X>`
    /// opens just that one psyop's browser.
    Browse {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, requires = "name")]
        commit: Option<String>,
    },
    /// Manage per-psyop target destinations.
    Targets {
        #[command(subcommand)]
        command: self::targets::Commands,
    },
    /// Sign in a psyop's X account. Requires the master X-App to
    /// already be signed in + fully set up (`x_app setup`). Opens
    /// the embedded browser scoped to `psyop/<name>/`; on sign-in
    /// the browser drives the OAuth 2.0 PKCE consent screen,
    /// exchanges the code, and writes auth.json under the psyop's
    /// data root. Refuses if the psyop is already signed in or
    /// already has an auth.json for the current X-App — pass
    /// `--dangerously-reset` to wipe its browser folder and re-login.
    #[command(name = "login")]
    Login {
        name: String,
        /// Wipe any existing browser state for this psyop before
        /// signing in. Required when re-logging in for a psyop that
        /// already has an active session or stored auth.json.
        #[arg(long)]
        dangerously_reset: bool,
    },
    /// Open the embedded browser as this psyop. Loads x.com under
    /// the psyop's CEF profile (shared with `psyops browse` /
    /// `psyops login`). No tweet-ID scraping, no OAuth, no
    /// twid-conflict guard — just a clean browser. The operator
    /// closes the window when done; the CLI blocks on that exit.
    /// The only mode hint shown is "Sign in to X" if not signed in.
    #[command(name = "browser")]
    Browser {
        name: String,
    },
}

impl Commands {
    pub async fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, Error> {
        match self {
            Commands::List { enabled, disabled } => psyops::list(enabled, disabled, cfg),
            Commands::Get { name } => psyops::get(&name, cfg),
            Commands::Enable { name, commit } => {
                psyops::set_disabled(&name, commit.as_deref(), false, cfg)
            }
            Commands::Disable { name, commit } => {
                psyops::set_disabled(&name, commit.as_deref(), true, cfg)
            }
            Commands::Publish { args } => psyops::publish(args, cfg),
            Commands::Delete { name } => psyops::delete(&name, cfg),
            Commands::Run { name, commit, seed } => {
                psyops::run::run_all(name.as_deref(), commit.as_deref(), seed, cfg).await
            }
            Commands::Browse { name, commit } => {
                psyops::browse::run(name.as_deref(), commit.as_deref(), cfg).await
            }
            Commands::Targets { command } => command.handle(cfg),
            Commands::Login { name, dangerously_reset } => {
                crate::login::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Psyop,
                    &name,
                    dangerously_reset,
                    cfg,
                )
                .await
            }
            Commands::Browser { name } => {
                crate::persona_browser::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Psyop,
                    &name,
                    cfg,
                )
                .await
            }
        }
    }
}
