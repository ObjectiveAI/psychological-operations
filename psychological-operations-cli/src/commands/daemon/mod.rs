//! `daemon` — the resident Discord gateway daemon.
//!
//! `daemon begin` is the entry the objectiveai daemon launches for this plugin
//! (manifest `daemon: true`, invoked as `<plugin-exec> daemon begin` with the
//! full bidirectional protocol — so Python runs through the normal
//! `PluginExecutor` in `ctx.executor`). It takes a process-singleton lock, then
//! opens a serenity gateway connection (all intents) for every agent that has
//! both Discord auth and one or more hooks, and runs each of that agent's hooks
//! for **every** gateway event. Never returns.
//!
//! A hook is operator Python (`discord_hooks`). The raw serenity event is
//! serialized to JSON and fed as the Python `input`; the result is ignored
//! (fire-and-forget). Hooks are snapshotted at start — adding/removing hooks
//! (or auth) requires restarting the daemon. Status goes to stderr; stdout is
//! the plugin protocol.

use std::sync::Arc;

use clap::Subcommand;
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path, Request};
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::discord::serenity;
use serenity::all::{Context as SerenityContext, Event, RawEventHandler};

use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// Run the resident Discord gateway daemon (never returns).
    Begin,
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        match self {
            Commands::Begin => crate::output::emit_result(begin(ctx).await),
        }
    }
}

/// Per-agent raw-event handler: runs every hook for every gateway event.
struct HookHandler {
    executor: Arc<PluginExecutor>,
    agent_tag: String,
    /// The agent's hooks' Python source, snapshotted at daemon start.
    hooks: Arc<Vec<String>>,
}

#[serenity::async_trait]
impl RawEventHandler for HookHandler {
    async fn raw_event(&self, _ctx: SerenityContext, ev: Event) {
        // The serenity event is the hook's `input`. Serialize once, share it
        // across the agent's hooks.
        let input = match serde_json::to_value(&ev) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "discord daemon [{}]: failed to serialize event: {e}",
                    self.agent_tag
                );
                return;
            }
        };
        for code in self.hooks.iter() {
            // Fire-and-forget: spawn so the gateway loop is never blocked, and
            // ignore the result (we only care that it runs).
            let executor = self.executor.clone();
            let req = Request {
                path_type: Path::Python,
                code: code.clone(),
                input: Some(input.clone()),
                base: Default::default(),
            };
            tokio::spawn(async move {
                let _ = python::execute(&*executor, req, None).await;
            });
        }
    }
}

async fn begin(ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Process-singleton: a second daemon would open duplicate gateway
    // connections for the same bots (which Discord punishes). Bow out if held.
    let lock_dir = ctx.config.state_dir().join("locks");
    let _claim = objectiveai_sdk::lockfile::try_acquire(
        &lock_dir,
        "discord-daemon",
        &format!("pid {} discord daemon", std::process::id()),
    )
    .await
    .ok_or_else(|| Error::Other("the Discord daemon is already running".into()))?;

    let client = psychological_operations_sdk::discord::Client::new(ctx.db.clone());
    let agents = ctx
        .db
        .discord_daemon_agents()
        .await
        .map_err(|e| Error::Other(format!("discord daemon: list agents: {e}")))?;
    eprintln!(
        "discord daemon: starting listeners for {} agent(s)",
        agents.len()
    );

    for tag in agents {
        let hooks: Vec<String> = ctx
            .db
            .discord_hook_list(&tag)
            .await
            .map_err(|e| Error::Other(format!("discord daemon: list hooks ({tag}): {e}")))?
            .into_iter()
            .map(|h| h.python)
            .collect();
        let n = hooks.len();
        let handler = HookHandler {
            executor: ctx.executor.clone(),
            agent_tag: tag.clone(),
            hooks: Arc::new(hooks),
        };
        client
            .gateway_raw(&tag, handler)
            .await
            .map_err(|e| Error::Other(format!("discord daemon: gateway ({tag}): {e}")))?;
        eprintln!("discord daemon: listener started for {tag} ({n} hook(s))");
    }

    // Never finish — keep the process (and its gateway connections) alive.
    std::future::pending::<()>().await;
    Ok(CliOutput::Ok)
}
