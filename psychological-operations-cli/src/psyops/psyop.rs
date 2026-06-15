//! Persistence for psyop definitions. The `PsyOp` struct + its
//! `validate` method live in
//! `psychological_operations_sdk::cli::psyops::psyop`; this file is the
//! load/save layer over the db crate's `psyops` table (name → JSONB
//! definition). Git/commit versioning was dropped — a psyop is keyed by
//! name only.

use super::PsyOp;

/// Read a psyop's definition by name. Errors with `PsyopNotFound` when
/// no such psyop exists.
pub async fn load(name: &str, ctx: &crate::context::Context) -> Result<PsyOp, crate::error::Error> {
    use crate::error::Error;
    let value = ctx
        .db
        .psyop_get(name)
        .await?
        .ok_or_else(|| Error::PsyopNotFound(name.to_string()))?;
    Ok(serde_json::from_value(value)?)
}

/// Insert or replace a psyop's definition.
pub async fn save(
    name: &str,
    psyop: &PsyOp,
    ctx: &crate::context::Context,
) -> Result<(), crate::error::Error> {
    let value = serde_json::to_value(psyop)?;
    ctx.db.psyop_upsert(name, &value).await?;
    Ok(())
}
