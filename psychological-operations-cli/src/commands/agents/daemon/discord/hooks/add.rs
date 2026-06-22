//! `agents daemon discord hooks add` — upsert a named Python hook for an agent.

use psychological_operations_db::{unix_now, DiscordHookEntry};
use psychological_operations_sdk::cli::Output as CliOutput;

use super::PythonSource;
use crate::error::Error;

pub async fn run(
    agent_tag: &str,
    name: &str,
    description: &str,
    source: PythonSource,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(agent_tag, name, description, source, ctx).await)
}

async fn run_inner(
    agent_tag: &str,
    name: &str,
    description: &str,
    source: PythonSource,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    let python = source.resolve()?;
    ctx.db
        .discord_hook_set(&DiscordHookEntry {
            agent_tag: agent_tag.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            python,
            updated_at: unix_now(),
        })
        .await
        .map_err(|e| Error::Other(format!("hook add: {e}")))?;
    Ok(CliOutput::Ok)
}
