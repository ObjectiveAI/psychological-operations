//! `targets` subcommand surface.
//!
//! Unified CRUD over destination lists + delivery-queue drain.
//! Every arm takes a mutually-exclusive 3-way **selector**:
//!
//! - `--global` — top-level `Config::targets` (or, for `deliver`,
//!   the whole queue).
//! - `--psyop <name>` — `Config::psyops[name].base.targets` (or,
//!   for `deliver`, every row for that psyop).
//! - `--psyop <name> --commit <sha>` —
//!   `Config::psyops[name].commits[sha].targets` (or, for
//!   `deliver`, rows for that psyop at that commit).
//!
//! `--commit` only valid alongside `--psyop`; exactly one of the
//! three forms is required. Per-arm bodies live in the sibling
//! files (`get.rs`, `add.rs`, `del.rs`, `deliver.rs`); this file
//! owns the clap surface, the selector type, and the dispatch.

use clap::{Args, Subcommand};
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

mod add;
mod del;
mod deliver;
mod list;

#[derive(Args)]
#[group(id = "selector", required = true, multiple = false)]
pub struct SelectorArgs {
    /// Operate on the top-level global targets list (or, for
    /// `deliver`, the entire delivery queue).
    #[arg(long, group = "selector")]
    global: bool,
    /// Operate on a specific psyop. Without `--commit`, that's
    /// the psyop's base layer (or, for `deliver`, every queued
    /// row for the psyop).
    #[arg(long, group = "selector", value_name = "NAME")]
    psyop: Option<String>,
    /// When combined with `--psyop`, narrows to that psyop's
    /// commit-specific overrides under `commits.<SHA>` (or, for
    /// `deliver`, queued rows whose `psyop_commit_sha` matches).
    /// Cannot be used with `--global` or on its own.
    #[arg(long, requires = "psyop", conflicts_with = "global", value_name = "SHA")]
    commit: Option<String>,
}

pub(super) enum Selector {
    Global,
    PsyopBase   { psyop: String },
    PsyopCommit { psyop: String, commit: String },
}

impl SelectorArgs {
    fn resolve(self) -> Result<Selector, Error> {
        match (self.global, self.psyop, self.commit) {
            (true,  None,    None)    => Ok(Selector::Global),
            (false, Some(p), None)    => Ok(Selector::PsyopBase { psyop: p }),
            (false, Some(p), Some(c)) => Ok(Selector::PsyopCommit { psyop: p, commit: c }),
            _ => Err(Error::Other(
                "exactly one of --global, --psyop, or --psyop+--commit is required".into(),
            )),
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// List the targets in the layer selected by --global /
    /// --psyop / --psyop+--commit. `--count` and `--offset`
    /// paginate the result (both omitted → entire list).
    List {
        #[command(flatten)]
        selector: SelectorArgs,
        /// Maximum entries to return. Omitted → no upper bound.
        #[arg(long)]
        count: Option<usize>,
        /// Skip the first N entries. Omitted → start at 0.
        #[arg(long)]
        offset: Option<usize>,
    },
    /// Append a target (Destination-shaped JSON) to the selected
    /// list.
    Add {
        #[command(flatten)]
        selector: SelectorArgs,
        json: String,
    },
    /// Remove the entry at `<index>` from the selected list.
    Del {
        #[command(flatten)]
        selector: SelectorArgs,
        index: usize,
    },
    /// Drain the delivery queue scoped by the selector: read
    /// every matching row, attempt redelivery, delete on success,
    /// bump-attempt on failure.
    Deliver {
        #[command(flatten)]
        selector: SelectorArgs,
    },
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                Commands::List { selector, count, offset } =>
                    list::run(selector.resolve()?, count, offset, ctx),
                Commands::Add { selector, json }   => add::run(selector.resolve()?, json, ctx),
                Commands::Del { selector, index }  => del::run(selector.resolve()?, index, ctx),
                Commands::Deliver { selector }     => deliver::run(selector.resolve()?, ctx).await,
            }
        }.await;
        crate::output::emit_result(result)
    }
}
