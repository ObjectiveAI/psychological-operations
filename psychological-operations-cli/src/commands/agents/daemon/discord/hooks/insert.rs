//! `agents daemon discord hooks insert {python,mention,reply,dm}` — add a named
//! hook for an agent. Replacing an existing hook of the same name requires
//! `--overwrite`. The chosen subcommand builds the typed `Hook`, which is stored
//! as JSONB.

use psychological_operations_db::{unix_now, DiscordHookEntry};
use psychological_operations_sdk::cli::hooks::Hook;
use psychological_operations_sdk::cli::Output as CliOutput;

use super::{CommonArgs, InsertHook};
use crate::error::Error;

pub async fn run(hook: InsertHook, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(hook, ctx).await)
}

async fn run_inner(hook: InsertHook, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Split each arm into its shared metadata + the typed `Hook` definition.
    let (common, hook): (CommonArgs, Hook) = match hook {
        InsertHook::Python { common, source } => (
            common,
            Hook::Python {
                code: source.resolve()?,
            },
        ),
        InsertHook::Mention {
            common,
            user_id,
            message,
        } => (common, Hook::Mention { user_id, message }),
        InsertHook::Reply {
            common,
            user_id,
            message,
        } => (common, Hook::Reply { user_id, message }),
        InsertHook::Dm {
            common,
            user_id,
            message,
        } => (common, Hook::Dm { user_id, message }),
    };

    hook.validate().map_err(Error::Other)?;

    let CommonArgs {
        agent_tag,
        name,
        description,
        overwrite,
    } = common;

    // Refuse to clobber an existing hook unless --overwrite was passed.
    if !overwrite && ctx.db.discord_hook_exists(&agent_tag, &name).await? {
        return Err(Error::Other(format!(
            "hook '{name}' already exists for agent '{agent_tag}' — pass --overwrite to replace it"
        )));
    }

    let definition =
        serde_json::to_value(&hook).map_err(|e| Error::Other(format!("serialize hook: {e}")))?;
    ctx.db
        .discord_hook_set(&DiscordHookEntry {
            agent_tag,
            name,
            description,
            definition,
            updated_at: unix_now(),
        })
        .await
        .map_err(|e| Error::Other(format!("hook insert: {e}")))?;
    // A running daemon reloads via the discord_hooks NOTIFY trigger — no
    // writer-side kick needed.
    Ok(CliOutput::Ok)
}
