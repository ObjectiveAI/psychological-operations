//! `agents daemon discord hooks delete` — remove a named hook from an agent.

use psychological_operations_sdk::cli::Output as CliOutput;

use crate::error::Error;

pub async fn run(agent_tag: &str, name: &str, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(agent_tag, name, ctx).await)
}

async fn run_inner(
    agent_tag: &str,
    name: &str,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    let removed = ctx
        .db
        .discord_hook_delete(agent_tag, name)
        .await
        .map_err(|e| Error::Other(format!("hook delete: {e}")))?;
    if !removed {
        return Err(Error::Other(format!(
            "no hook named '{name}' for agent '{agent_tag}'"
        )));
    }
    // Tell a running daemon to reload (no-op if none is running).
    crate::commands::daemon::request_reload(&ctx.config.state_dir()).await?;
    Ok(CliOutput::Ok)
}
