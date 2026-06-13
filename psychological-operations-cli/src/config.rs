//! Delivery-target configuration.
//!
//! Targets used to live in `config.json`; they now live in postgres
//! (`global_targets` / `psyop_targets`), with the per-psyop `disabled`
//! flag on the `psyops` row. This module is the typed facade over the
//! db crate's target methods: it (de)serializes the CLI's [`Destination`]
//! to/from the JSONB the db stores, and owns the first-run default set.

use serde_json::Value;

use crate::context::Context;
use crate::error::Error;
use crate::targets::destinations::{stdout, x, Destination};

/// The out-of-the-box global targets seeded on first run: like every
/// scoring survivor on X, and emit each delivery to stdout as a
/// structured event.
pub fn default_targets() -> Vec<Destination> {
    vec![
        Destination::X(x::X { r#type: x::XType::Like }),
        Destination::Stdout(stdout::Stdout::default()),
    ]
}

/// Seed the default global targets exactly once (first run). No-op
/// afterwards, even if the operator later empties the list. Called once
/// per process from [`Context::new`].
pub async fn seed_defaults(ctx: &Context) -> Result<(), Error> {
    let defaults: Vec<Value> = default_targets()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    ctx.db.seed_global_targets_if_unseeded(&defaults).await?;
    Ok(())
}

/// Global targets, decoded to [`Destination`].
pub async fn global_targets(ctx: &Context) -> Result<Vec<Destination>, Error> {
    decode_targets(ctx.db.global_targets().await?)
}

/// Replace the global target list.
pub async fn set_global_targets(ctx: &Context, targets: &[Destination]) -> Result<(), Error> {
    ctx.db.set_global_targets(&encode_targets(targets)?).await?;
    Ok(())
}

/// One psyop's targets, decoded to [`Destination`].
pub async fn psyop_targets(ctx: &Context, psyop: &str) -> Result<Vec<Destination>, Error> {
    decode_targets(ctx.db.psyop_targets(psyop).await?)
}

/// Replace one psyop's target list.
pub async fn set_psyop_targets(
    ctx: &Context,
    psyop: &str,
    targets: &[Destination],
) -> Result<(), Error> {
    ctx.db
        .set_psyop_targets(psyop, &encode_targets(targets)?)
        .await?;
    Ok(())
}

fn decode_targets(values: Vec<Value>) -> Result<Vec<Destination>, Error> {
    values
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(Error::from))
        .collect()
}

fn encode_targets(targets: &[Destination]) -> Result<Vec<Value>, Error> {
    targets
        .iter()
        .map(|d| serde_json::to_value(d).map_err(Error::from))
        .collect()
}
