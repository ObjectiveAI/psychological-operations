//! Persistence for psyop definitions. The `PsyOp` struct + its
//! `validate` method live in
//! `psychological_operations_sdk::cli::psyops::psyop`; this file is the
//! load/save layer over the db crate's `psyops` table (name → JSONB
//! definition). Git/commit versioning was dropped — a psyop is keyed by
//! name only.

use super::PsyOp;

/// Read a psyop's definition by name as the polymorphic [`PsyOp`] (either
/// family — the untagged enum picks X vs Discord by the body's `type`).
/// Errors with `PsyopNotFound` when no such psyop exists.
pub async fn load(name: &str, ctx: &crate::context::Context) -> Result<PsyOp, crate::error::Error> {
    use crate::error::Error;
    let value = ctx
        .db
        .psyop_get(name)
        .await?
        .ok_or_else(|| Error::PsyopNotFound(name.to_string()))?;
    Ok(serde_json::from_value(value)?)
}

/// Insert or replace a psyop's definition (any family).
pub async fn save(
    name: &str,
    psyop: &PsyOp,
    ctx: &crate::context::Context,
) -> Result<(), crate::error::Error> {
    let value = serde_json::to_value(psyop)?;
    // Denormalize the trigger interval (seconds) into its own column so the
    // daemon scheduler can compute the next-due time in SQL. Manual triggers
    // (and, defensively, an unparseable interval that validation should already
    // have rejected) store NULL — never auto-run.
    let interval_secs = match psyop.trigger_interval() {
        Ok(Some(d)) => Some(d.as_secs() as i64),
        _ => None,
    };
    ctx.db.psyop_upsert(name, &value, interval_secs).await?;
    Ok(())
}
