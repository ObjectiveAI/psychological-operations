//! `agents daemon twitch hooks list` — list an agent's hooks (name + type +
//! description; the definition body is not surfaced).

use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::cli::output::TwitchHookEntry;

use crate::error::Error;

pub async fn run(agent_tag: &str, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(agent_tag, ctx).await)
}

async fn run_inner(agent_tag: &str, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    let hooks = ctx
        .db
        .twitch_hook_list(agent_tag)
        .await
        .map_err(|e| Error::Other(format!("hook list: {e}")))?;
    let entries = hooks
        .into_iter()
        .map(|h| TwitchHookEntry {
            name: h.name,
            // The hook's `type` discriminator lives inside the JSONB definition.
            hook_type: h
                .definition
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string(),
            description: h.description,
        })
        .collect();
    Ok(CliOutput::TwitchHookList(entries))
}
