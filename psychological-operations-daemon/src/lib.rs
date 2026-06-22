//! Discord gateway daemon.
//!
//! [`run`] opens a gateway connection for every agent that has BOTH Discord
//! auth and one or more hooks ([`Db::discord_daemon_agents`]), and runs each
//! of that agent's hooks for **every** gateway event the connection receives.
//!
//! A hook is operator-defined Python (stored in `discord_hooks`). The raw
//! serenity [`Event`] is serialized to JSON and fed to the hook as the Python
//! global `input`; the hook's result is ignored (fire-and-forget). Python runs
//! through the supplied [`CommandExecutor`] — in practice the CLI's
//! `PluginExecutor`, since the daemon runs as a host-launched `daemon begin`
//! plugin with the full bidirectional protocol.
//!
//! Hooks are **snapshotted at start**: adding/removing hooks (or auth) requires
//! restarting the daemon. `run` never returns — the per-agent gateway loops run
//! in background tasks.

use std::sync::Arc;

use objectiveai_sdk::cli::command::CommandExecutor;
use objectiveai_sdk::cli::command::python::{self, Path, Request};
use psychological_operations_db::Db;
use psychological_operations_sdk::discord::serenity;
use serenity::all::{Context as SerenityContext, Event, RawEventHandler};

/// Daemon startup errors (listing agents / opening a gateway). Per-event hook
/// failures are not errors here — they're logged and ignored.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("db error: {0}")]
    Db(#[from] psychological_operations_db::Error),
    #[error("discord error: {0}")]
    Discord(#[from] psychological_operations_sdk::discord::Error),
}

/// One hook snapshotted at daemon start.
struct Hook {
    name: String,
    python: String,
}

/// Per-agent raw-event handler: runs every hook for every gateway event.
struct HookHandler<E> {
    executor: E,
    agent_tag: String,
    hooks: Arc<Vec<Hook>>,
}

#[serenity::async_trait]
impl<E> RawEventHandler for HookHandler<E>
where
    E: CommandExecutor + Clone + Send + Sync + 'static,
{
    async fn raw_event(&self, _ctx: SerenityContext, ev: Event) {
        // The serenity event is the hook's `input`. Serialize once, share
        // across hooks.
        let input = match serde_json::to_value(&ev) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(agent = %self.agent_tag, "failed to serialize event: {e}");
                return;
            }
        };
        for hook in self.hooks.iter() {
            // Fire-and-forget: spawn so the gateway loop is never blocked, and
            // ignore the result (we only care that it runs).
            let executor = self.executor.clone();
            let req = Request {
                path_type: Path::Python,
                code: hook.python.clone(),
                input: Some(input.clone()),
                base: Default::default(),
            };
            tokio::spawn(async move {
                let _ = python::execute(&executor, req, None).await;
            });
        }
    }
}

/// Run the Discord daemon forever: start a gateway listener per eligible agent,
/// then block. The gateway loops run in background tasks.
pub async fn run<E>(db: Db, executor: E) -> Result<(), Error>
where
    E: CommandExecutor + Clone + Send + Sync + 'static,
{
    let client = psychological_operations_sdk::discord::Client::new(db.clone());
    let agents = db.discord_daemon_agents().await?;
    tracing::info!(
        "discord daemon: starting listeners for {} agent(s)",
        agents.len()
    );

    for tag in agents {
        let hooks: Vec<Hook> = db
            .discord_hook_list(&tag)
            .await?
            .into_iter()
            .map(|h| Hook {
                name: h.name,
                python: h.python,
            })
            .collect();
        let n = hooks.len();
        let handler = HookHandler {
            executor: executor.clone(),
            agent_tag: tag.clone(),
            hooks: Arc::new(hooks),
        };
        client.gateway_raw(&tag, handler).await?;
        tracing::info!(agent = %tag, hooks = n, "discord daemon: listener started");
    }

    // Never finish — keep the process (and its gateway connections) alive.
    std::future::pending::<()>().await;
    Ok(())
}
