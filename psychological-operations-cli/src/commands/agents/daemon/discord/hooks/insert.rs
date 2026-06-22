//! `agents daemon discord hooks insert` — add a named Python hook for an
//! agent. Replacing an existing hook of the same name requires `--overwrite`.

use psychological_operations_db::{unix_now, DiscordHookEntry};
use psychological_operations_sdk::cli::Output as CliOutput;

use super::PythonSource;
use crate::error::Error;

pub async fn run(
    agent_tag: &str,
    name: &str,
    description: &str,
    overwrite: bool,
    source: PythonSource,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_inner(agent_tag, name, description, overwrite, source, ctx).await)
}

async fn run_inner(
    agent_tag: &str,
    name: &str,
    description: &str,
    overwrite: bool,
    source: PythonSource,
    ctx: &crate::context::Context,
) -> Result<CliOutput, Error> {
    // Refuse to clobber an existing hook unless --overwrite was passed.
    if !overwrite && ctx.db.discord_hook_exists(agent_tag, name).await? {
        return Err(Error::Other(format!(
            "hook '{name}' already exists for agent '{agent_tag}' — pass --overwrite to replace it"
        )));
    }
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
        .map_err(|e| Error::Other(format!("hook insert: {e}")))?;
    // A running daemon reloads via the discord_hooks NOTIFY trigger — no
    // writer-side kick needed.
    Ok(CliOutput::Ok)
}
