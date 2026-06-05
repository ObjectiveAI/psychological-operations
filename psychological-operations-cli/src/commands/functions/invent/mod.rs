//! `functions invent` subcommand surface.
//!
//! The clap `Args` structs come straight from
//! `objectiveai_sdk::cli::command::functions::inventions::recursive::create::{alpha_scalar, alpha_vector, remote}`
//! — embedding them here is what keeps the psyops surface in
//! lockstep with objectiveai's verbatim. The dispatch in
//! [`Commands::handle`] hands each parsed `Args` off to the
//! per-leaf entry points in `crate::invent`, which do the schema
//! injection + recursive-remote dispatch.

use clap::Subcommand;
use objectiveai_sdk::cli::command::functions::inventions::recursive::create::{
    alpha_scalar, alpha_vector, remote,
};

#[derive(Subcommand)]
pub enum Commands {
    /// Invent a scalar function for scoring individual posts.
    /// Args mirror `objectiveai functions inventions recursive
    /// create alpha-scalar` verbatim. Psyops auto-applies its
    /// post input-schema.
    #[command(name = "alpha-scalar")]
    AlphaScalar(alpha_scalar::Args),
    /// Invent a vector function for ranking posts. Args mirror
    /// `objectiveai functions inventions recursive create
    /// alpha-vector` verbatim. Psyops auto-applies its post
    /// input-schema.
    #[command(name = "alpha-vector")]
    AlphaVector(alpha_vector::Args),
    /// Invent from an existing state (remote reference or inline
    /// JSON). Args mirror `objectiveai functions inventions
    /// recursive create remote` verbatim. Psyops auto-applies
    /// its post input-schema to any state variant that lacks
    /// one.
    Remote(remote::Args),
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::AlphaScalar(args) => crate::invent::invent_alpha_scalar(args, ctx).await,
            Commands::AlphaVector(args) => crate::invent::invent_alpha_vector(args, ctx).await,
            Commands::Remote(args)      => crate::invent::invent_remote(args, ctx).await,
        }
    }
}
