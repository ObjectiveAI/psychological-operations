//! `agents daemon twitch hooks insert {python,mention}` — add a named hook for
//! an agent. Replacing an existing hook of the same name requires `--overwrite`.
//! The chosen subcommand builds the typed `TwitchHook`, which is stored as JSONB.

use objectiveai_sdk::cli::command::agents::tags::lookup as tags_lookup;
use psychological_operations_db::{unix_now, TwitchHookEntry};
use psychological_operations_sdk::cli::hooks::TwitchHook;
use psychological_operations_sdk::cli::Output as CliOutput;

use super::{CommonArgs, InsertHook};
use crate::error::Error;

pub async fn run(hook: InsertHook, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_inner(hook, ctx).await)
}

async fn run_inner(hook: InsertHook, ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Split each arm into its shared metadata + the typed `TwitchHook` definition.
    let (common, hook): (CommonArgs, TwitchHook) = match hook {
        InsertHook::Python { common, source } => (
            common,
            TwitchHook::Python {
                code: source.resolve()?,
            },
        ),
        InsertHook::Mention {
            common,
            user_login,
            message,
        } => {
            // Default the watched login to the caller's (the invoking agent's)
            // Twitch login. Best-effort — if it can't be resolved the field
            // stays `None` and the daemon falls back to the hook's own agent
            // login.
            let user_login = match user_login {
                Some(l) => Some(l),
                None => resolve_caller_login(ctx).await,
            };
            (common, TwitchHook::Mention { user_login, message })
        }
    };

    hook.validate().map_err(Error::Other)?;

    let CommonArgs {
        agent_tag,
        name,
        description,
        overwrite,
    } = common;

    // Refuse to clobber an existing hook unless --overwrite was passed.
    if !overwrite && ctx.db.twitch_hook_exists(&agent_tag, &name).await? {
        return Err(Error::Other(format!(
            "hook '{name}' already exists for agent '{agent_tag}' — pass --overwrite to replace it"
        )));
    }

    let definition =
        serde_json::to_value(&hook).map_err(|e| Error::Other(format!("serialize hook: {e}")))?;
    ctx.db
        .twitch_hook_set(&TwitchHookEntry {
            agent_tag,
            name,
            description,
            definition,
            updated_at: unix_now(),
        })
        .await
        .map_err(|e| Error::Other(format!("hook insert: {e}")))?;
    // A running daemon reloads via the twitch_hooks NOTIFY trigger — no
    // writer-side kick needed.
    Ok(CliOutput::Ok)
}

/// Resolve the caller's (the invoking objectiveai agent's) Twitch login, for
/// defaulting a `mention` hook's watched login. Best-effort: any missing piece
/// — no caller hierarchy, no tag bound to it, or that tag has no Twitch auth —
/// yields `None`, and the daemon then falls back to the hook's own agent login.
///
/// The caller AIH is split on its last `/` the same way `mcp twitch begin` does;
/// `agents tags lookup` returns the tags bound to that instance (newest first),
/// and the first tag's `twitch_auth.login` is the caller's login.
async fn resolve_caller_login(ctx: &crate::context::Context) -> Option<String> {
    let caller_aih = &ctx.config.objectiveai_agent_instance_hierarchy;
    let (parent, agent_instance) = caller_aih.rsplit_once('/')?;
    let request = tags_lookup::Request::AgentInstanceHierarchy {
        path_type: tags_lookup::Path::AgentsTagsLookup,
        parent_agent_instance_hierarchy: Some(parent.to_string()),
        agent_instance: agent_instance.to_string(),
        base: Default::default(),
    };
    let response = tags_lookup::execute(&*ctx.executor, request, None)
        .await
        .ok()?;
    let tag = match response {
        tags_lookup::Response::AgentInstanceHierarchy { tags } => tags.into_iter().next()?,
        _ => return None,
    };
    ctx.db
        .twitch_auth_get(&tag)
        .await
        .ok()
        .flatten()
        .and_then(|a| a.login)
}
