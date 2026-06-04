//! `invent` subcommand surface.
//!
//! `InventionParams`, `ForwardArgs`, `run_invention`, `fetch_state`,
//! and `fill_schema_if_missing` live in `crate::invent`. This file
//! owns the clap surface and the dispatch that constructs the
//! `ParamsState` envelope before delegating.

use clap::Subcommand;
use objectiveai_sdk::functions::inventions::{
    ParamsState,
    state::{AlphaScalarState, AlphaVectorState},
};
use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::input;
use crate::invent::{ForwardArgs, InventionParams};

#[derive(Subcommand)]
pub enum Commands {
    /// Invent a scalar function for scoring individual posts
    AlphaScalar {
        #[command(flatten)]
        params: InventionParams,
        #[command(flatten)]
        forward: ForwardArgs,
    },
    /// Invent a vector function for ranking posts
    AlphaVector {
        #[command(flatten)]
        params: InventionParams,
        #[command(flatten)]
        forward: ForwardArgs,
    },
    /// Invent from existing state (remote reference or inline JSON)
    Remote {
        /// State reference (e.g. remote=mock,name=inv-good-sl)
        #[arg(long, required_unless_present = "state_inline")]
        state: Option<String>,
        /// Inline JSON state
        #[arg(long, conflicts_with = "state")]
        state_inline: Option<String>,
        #[command(flatten)]
        forward: ForwardArgs,
    },
}

impl Commands {
    pub async fn handle(self, _cfg: &crate::run::Config) -> Result<Output, Error> {
        match self {
            Commands::AlphaScalar { params, forward } => {
                let p = params.into_params();
                let state = ParamsState::AlphaScalar(AlphaScalarState {
                    params: p,
                    input_schema: Some(input::scalar_input_schema()),
                });
                crate::invent::run_invention(&state, &forward).await
            }
            Commands::AlphaVector { params, forward } => {
                let p = params.into_params();
                let state = ParamsState::AlphaVector(AlphaVectorState {
                    params: p,
                    input_schema: Some(input::vector_input_schema()),
                });
                crate::invent::run_invention(&state, &forward).await
            }
            Commands::Remote { state, state_inline, forward } => {
                let resolved = if let Some(inline) = state_inline {
                    let parsed: ParamsState = serde_json::from_str(&inline)?;
                    crate::invent::fill_schema_if_missing(parsed)
                } else if let Some(ref ref_str) = state {
                    let fetched = crate::invent::fetch_state(ref_str).await?;
                    crate::invent::fill_schema_if_missing(fetched)
                } else {
                    return Err(Error::Other(
                        "--state or --state-inline is required".into(),
                    ));
                };
                crate::invent::run_invention(&resolved, &forward).await
            }
        }
    }
}
