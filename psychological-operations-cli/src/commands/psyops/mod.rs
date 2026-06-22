//! `psyops` subcommand surface.
//!
//! The type defs (`PsyOp`, `PsyopSource`, `PublishArgs`, ...), the
//! run loop, the browse helpers, and the per-command body fns
//! (`list`, `get`, `set_disabled`, `insert`, `delete`) all stay in
//! `crate::psyops`. This file owns the clap surface and the dispatch
//! that calls into them.

use clap::Subcommand;

use crate::psyops::{self, PublishArgs};

#[derive(Subcommand)]
pub enum Commands {
    /// List all psyops. `--enabled` / `--disabled` are mutually exclusive
    /// state filters; `--x` / `--discord` are mutually exclusive family
    /// filters. `--count` / `--offset` paginate (both omitted → entire list).
    List {
        #[arg(long, conflicts_with = "disabled")]
        enabled: bool,
        #[arg(long)]
        disabled: bool,
        /// Only X psyops.
        #[arg(long, conflicts_with = "discord")]
        x: bool,
        /// Only Discord psyops.
        #[arg(long)]
        discord: bool,
        /// Maximum entries to return. Omitted → no upper bound.
        #[arg(long)]
        count: Option<usize>,
        /// Skip the first N entries. Omitted → start at 0.
        #[arg(long)]
        offset: Option<usize>,
    },
    /// Print the on-disk JSON definition of a psyop.
    Get { name: String },
    /// Emit the JSON Schema for a PsyOp definition — the shape
    /// `insert --psyop-inline '<json>'` accepts.
    Schema,
    /// Mark a psyop as enabled.
    Enable { name: String },
    /// Mark a psyop as disabled.
    Disable { name: String },
    /// Insert a psyop definition (upserts it by name).
    Insert {
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
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::List {
                enabled,
                disabled,
                x,
                discord,
                count,
                offset,
            } => psyops::list(enabled, disabled, x, discord, count, offset, ctx).await,
            Commands::Get { name } => psyops::get(&name, ctx).await,
            Commands::Schema => psyops::schema(),
            Commands::Enable { name } => psyops::set_disabled(&name, false, ctx).await,
            Commands::Disable { name } => psyops::set_disabled(&name, true, ctx).await,
            Commands::Insert { args } => psyops::insert(args, ctx).await,
            Commands::Run { name, seed } => psyops::run::run_all(name, seed, ctx).await,
        }
    }
}
