//! `agents daemon twitch hooks get` — show one hook's full typed definition
//! (name + description + the `TwitchHook` enum body).

use psychological_operations_sdk::cli::hooks::TwitchHook;
use psychological_operations_sdk::cli::output::TwitchHookFull;
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
    let entry = ctx
        .db
        .twitch_hook_get(agent_tag, name)
        .await
        .map_err(|e| Error::Other(format!("hook get: {e}")))?
        .ok_or_else(|| Error::Other(format!("no hook named '{name}' for agent '{agent_tag}'")))?;
    let hook: TwitchHook = serde_json::from_value(entry.definition)
        .map_err(|e| Error::Other(format!("hook '{name}' malformed: {e}")))?;
    Ok(CliOutput::TwitchHook(TwitchHookFull {
        name: entry.name,
        description: entry.description,
        definition: hook,
    }))
}
