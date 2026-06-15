//! `psyops` subcommand surface.
//!
//! The type defs (`PsyOp`, `PsyopSource`, `PublishArgs`, ...), the
//! run loop, the browse helpers, and the per-command body fns
//! (`list`, `get`, `set_disabled`, `publish`, `delete`) all stay in
//! `crate::psyops`. This file owns the clap surface and the dispatch
//! that calls into them.

use clap::Subcommand;

use crate::psyops::{self, PublishArgs};

#[derive(Subcommand)]
pub enum Commands {
    /// List all psyops on disk. `enabled` reflects the resolved state at
    /// each psyop's current commit. `--enabled` and `--disabled` are
    /// mutually exclusive filters. `--count` / `--offset` paginate
    /// the result (both omitted → entire list).
    List {
        #[arg(long, conflicts_with = "disabled")]
        enabled: bool,
        #[arg(long)]
        disabled: bool,
        /// Maximum entries to return. Omitted → no upper bound.
        #[arg(long)]
        count: Option<usize>,
        /// Skip the first N entries. Omitted → start at 0.
        #[arg(long)]
        offset: Option<usize>,
    },
    /// Print the on-disk JSON definition of a psyop.
    Get {
        name: String,
    },
    /// Emit the JSON Schema for a PsyOp definition — the shape
    /// `publish --psyop-inline '<json>'` accepts.
    Schema,
    /// Mark a psyop as enabled.
    Enable {
        name: String,
    },
    /// Mark a psyop as disabled.
    Disable {
        name: String,
    },
    /// Publish a psyop definition (upserts it by name).
    Publish {
        #[command(flatten)]
        args: PublishArgs,
    },
    /// Run psyops end-to-end. Repeat `--name X --name Y` to run exactly
    /// those psyops; omit `--name` entirely to run every psyop that can
    /// run right now. In both cases a psyop is skipped unless its
    /// `interval` has elapsed since its last successful run.
    Run {
        #[arg(long)]
        name: Vec<String>,
        /// Pass-through to `objectiveai` for deterministic mock
        /// outputs. Used by integration tests; optional otherwise.
        #[arg(long)]
        seed: Option<i64>,
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
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::List { enabled, disabled, count, offset } =>
                psyops::list(enabled, disabled, count, offset, ctx).await,
            Commands::Get { name } => psyops::get(&name, ctx).await,
            Commands::Schema => psyops::schema(),
            Commands::Enable { name } => {
                psyops::set_disabled(&name, false, ctx).await
            }
            Commands::Disable { name } => {
                psyops::set_disabled(&name, true, ctx).await
            }
            Commands::Publish { args } => psyops::publish(args, ctx).await,
            Commands::Run { name, seed } => {
                psyops::run::run_all(name, seed, ctx).await
            }
            Commands::Login { name, dangerously_reset } => {
                crate::login::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Psyop,
                    &name,
                    dangerously_reset,
                    ctx,
                )
                .await
            }
            Commands::Browser { name } => {
                crate::persona_browser::run(
                    psychological_operations_sdk::browser::auth_json::PersonaKind::Psyop,
                    &name,
                    ctx,
                )
                .await
            }
        }
    }
}
