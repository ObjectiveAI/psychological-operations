//! `agents daemon discord hooks list` — list an agent's hooks (name +
//! description; the Python source is not surfaced).

use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::cli::output::DiscordHookEntry;

use crate::error::Error;

pub async fn run(agent_tag: &str, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(agent_tag, ctx).await)
}

async fn run_inner(agent_tag: &str, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    let hooks = ctx
        .db
        .discord_hook_list(agent_tag)
        .await
        .map_err(|e| Error::Other(format!("hook list: {e}")))?;
    let entries = hooks
        .into_iter()
        .map(|h| DiscordHookEntry {
            name: h.name,
            description: h.description,
        })
        .collect();
    Ok(CliOutput::DiscordHookList(entries))
}
