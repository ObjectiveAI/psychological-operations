//! `targets` subcommand surface.
//!
//! Unified CRUD over destination lists + delivery-queue drain.
//! Every arm takes a mutually-exclusive 2-way **selector**:
//!
//! - `--global` — the `global_targets` table (or, for `deliver`,
//!   the whole queue).
//! - `--psyop <name>` — the `psyop_targets` rows for that psyop (or,
//!   for `deliver`, every row for that psyop).
//!
//! Exactly one of the two forms is required. Per-arm bodies live in
//! the sibling files (`add.rs`, `del.rs`, `list.rs`, `deliver.rs`);
//! this file owns the clap surface, the selector type, and dispatch.

use clap::{Args, Subcommand};
use psychological_operations_sdk::cli::Output;

use crate::error::Error;

mod add;
mod del;
mod deliver;
mod list;
mod schema;

#[derive(Args)]
#[group(id = "selector", required = true, multiple = false)]
pub struct SelectorArgs {
    /// Operate on the global targets list (or, for `deliver`, the
    /// entire delivery queue).
    #[arg(long, group = "selector")]
    global: bool,
    /// Operate on a specific psyop's targets (or, for `deliver`,
    /// every queued row for the psyop).
    #[arg(long, group = "selector", value_name = "NAME")]
    psyop: Option<String>,
}

pub(super) enum Selector {
    Global,
    Psyop { psyop: String },
}

impl SelectorArgs {
    fn resolve(self) -> Result<Selector, Error> {
        match (self.global, self.psyop) {
            (true, None) => Ok(Selector::Global),
            (false, Some(p)) => Ok(Selector::Psyop { psyop: p }),
            _ => Err(Error::Other(
                "exactly one of --global or --psyop is required".into(),
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
    /// Emit the JSON Schema for a Destination — the JSON body
    /// `targets add <selector> '<json>'` accepts.
    Schema,
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        let result: Result<Output, Error> = async move {
            match self {
                Commands::List { selector, count, offset } =>
                    list::run(selector.resolve()?, count, offset, ctx).await,
                Commands::Add { selector, json }   => add::run(selector.resolve()?, json, ctx).await,
                Commands::Del { selector, index }  => del::run(selector.resolve()?, index, ctx).await,
                Commands::Deliver { selector }     => deliver::run(selector.resolve()?, ctx).await,
                Commands::Schema                   => schema::run(),
            }
        }.await;
        crate::output::emit_result(result)
    }
}
