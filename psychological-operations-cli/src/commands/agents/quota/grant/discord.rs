//! `agents quota grant discord` — additive Discord quota boost, against the
//! Discord MCP's separate budget (`discord_quota_grants`).

use psychological_operations_db::unix_now;
use psychological_operations_sdk::cli::Output;

use super::Direction;
use crate::error::Error;

pub async fn run(
    ctx: &crate::context::Context,
    mode: Direction,
    agent_tag: &str,
    quantity: u64,
    duration: &str,
) -> bool {
    crate::output::emit_result(run_inner(ctx, mode, agent_tag, quantity, duration).await)
}

async fn run_inner(
    ctx: &crate::context::Context,
    mode: Direction,
    agent_tag: &str,
    quantity: u64,
    duration: &str,
) -> Result<Output, Error> {
    let secs = humantime::parse_duration(duration)
        .map_err(|e| Error::Other(format!("invalid duration: {e}")))?
        .as_secs() as i64;
    let granted_at = unix_now();
    let expires_at = granted_at + secs;
    ctx.db
        .grant_discord_quota(agent_tag, mode.as_str(), quantity as i64, granted_at, expires_at)
        .await
        .map_err(|e| Error::Other(format!("discord quota grant: {e}")))?;
    Ok(Output::Ok)
}
