//! `daemon` — the resident Discord gateway daemon + its reload socket.
//!
//! `daemon begin` is the entry the objectiveai daemon launches for this plugin
//! (manifest `daemon: true`, invoked as `<plugin-exec> daemon begin` with the
//! full bidirectional protocol — so Python runs through the normal
//! `PluginExecutor` in `ctx.executor`). It takes a process-singleton lock,
//! loads the DB, then listens on a cross-platform local socket (interprocess)
//! for **reload** requests.
//!
//! Hooks are held in a shared, live store rather than snapshotted: a reload
//! re-queries the DB, swaps the store (so running listeners pick up the new
//! hooks immediately), and starts gateway listeners for any newly-eligible
//! agents (`gateway_raw` is idempotent per agent). An agent that loses
//! eligibility goes quiet (its handler finds no hooks) but keeps its
//! connection. The reload acks `ok` or `err <message>`.
//!
//! [`request_reload`] is the client side, used by the mutating commands. It's
//! best-effort: a daemon that isn't running (connect fails) is not an error —
//! only an explicit `err` ack is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Subcommand;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use objectiveai_sdk::cli::command::python::{self, Path as PyPath, Request};
use psychological_operations_sdk::cli::Output as CliOutput;
use psychological_operations_sdk::discord::{self, serenity};
use serenity::all::{Context as SerenityContext, Event, RawEventHandler};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;

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

/// agent_tag -> the agent's hooks' Python sources. Shared between the gateway
/// handlers (read on every event) and reload (replaces it wholesale).
type HookStore = Arc<RwLock<HashMap<String, Arc<Vec<String>>>>>;

/// The reload socket lives at a fixed path inside the (per-state) state dir.
/// The daemon's singleton lock already guarantees one per state, so no
/// disambiguation is needed. `GenericFilePath` is supported on every platform
/// (a Unix-domain socket file, or a named pipe derived from the path on
/// Windows).
fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join("discord-daemon.sock")
}

/// Per-agent raw-event handler: runs every hook for every gateway event,
/// reading the current hooks from the shared live store.
struct HookHandler {
    executor: Arc<PluginExecutor>,
    store: HookStore,
    agent_tag: String,
}

#[serenity::async_trait]
impl RawEventHandler for HookHandler {
    async fn raw_event(&self, _ctx: SerenityContext, ev: Event) {
        // Latest hooks for this agent (cloned out so we don't hold the lock).
        let Some(codes) = self.store.read().await.get(&self.agent_tag).cloned() else {
            return;
        };
        if codes.is_empty() {
            return;
        }
        // The serenity event is the hook's `input`. Serialize once.
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
        for code in codes.iter() {
            // Fire-and-forget: spawn so the gateway loop isn't blocked, and
            // ignore the result (we only care that it runs).
            let executor = self.executor.clone();
            let req = Request {
                path_type: PyPath::Python,
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

/// Re-query the DB and apply: swap the hook store, then ensure a gateway
/// listener exists for every eligible agent (`gateway_raw` is idempotent, so
/// existing listeners are no-ops and new agents get a fresh one).
async fn do_reload(
    db: &psychological_operations_db::Db,
    executor: &Arc<PluginExecutor>,
    store: &HookStore,
    client: &discord::Client,
) -> Result<(), Error> {
    let agents = db
        .discord_daemon_agents()
        .await
        .map_err(|e| Error::Other(format!("list agents: {e}")))?;

    // Agents we had a listener for last reload (the store's keys) that are no
    // longer eligible — tear their gateway connections down.
    let to_drop: Vec<String> = {
        let cur = store.read().await;
        cur.keys()
            .filter(|k| !agents.contains(k))
            .cloned()
            .collect()
    };

    let mut map: HashMap<String, Arc<Vec<String>>> = HashMap::with_capacity(agents.len());
    for tag in &agents {
        let codes: Vec<String> = db
            .discord_hook_list(tag)
            .await
            .map_err(|e| Error::Other(format!("list hooks ({tag}): {e}")))?
            .into_iter()
            .map(|h| h.python)
            .collect();
        map.insert(tag.clone(), Arc::new(codes));
    }
    *store.write().await = map;

    for tag in to_drop {
        client.stop_gateway(&tag).await;
        eprintln!("discord daemon: dropped listener for {tag}");
    }

    for tag in agents {
        let handler = HookHandler {
            executor: executor.clone(),
            store: store.clone(),
            agent_tag: tag.clone(),
        };
        client
            .gateway_raw(&tag, handler)
            .await
            .map_err(|e| Error::Other(format!("gateway ({tag}): {e}")))?;
    }
    Ok(())
}

async fn begin(ctx: &crate::context::Context) -> Result<CliOutput, Error> {
    // Process-singleton: a second daemon would open duplicate gateway
    // connections for the same bots (which Discord punishes). Bow out if held.
    let state_dir = ctx.config.state_dir();
    let _claim = objectiveai_sdk::lockfile::try_acquire(
        &state_dir.join("locks"),
        "daemon",
        &format!("pid {} discord daemon", std::process::id()),
    )
    .await
    .ok_or_else(|| Error::Other("the Discord daemon is already running".into()))?;

    let store: HookStore = Arc::new(RwLock::new(HashMap::new()));
    let client = discord::Client::new(ctx.db.clone());

    // Initial load.
    do_reload(&ctx.db, &ctx.executor, &store, &client).await?;
    eprintln!("discord daemon: initial load complete");

    // Bind the reload socket (fixed path in the state dir). Remove any stale
    // socket file left by a crashed predecessor — safe since we hold the
    // singleton lock. (On Windows the path maps to a named pipe; the remove is
    // a harmless no-op.)
    let path = socket_path(&state_dir);
    let _ = std::fs::remove_file(&path);
    let name = path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| Error::Other(format!("reload socket name: {e}")))?;
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .map_err(|e| Error::Other(format!("bind reload socket: {e}")))?;
    eprintln!("discord daemon: listening for reload requests");

    // Serve reload requests forever (one connection at a time, so reloads
    // never race the store swap).
    loop {
        let conn = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("discord daemon: accept error: {e}");
                continue;
            }
        };
        let (recv, mut send) = conn.split();
        // Read the request line (any line means "reload").
        let mut lines = BufReader::new(recv).lines();
        let _ = lines.next_line().await;
        let resp = match do_reload(&ctx.db, &ctx.executor, &store, &client).await {
            Ok(()) => {
                eprintln!("discord daemon: reloaded");
                "ok\n".to_string()
            }
            Err(e) => {
                eprintln!("discord daemon: reload failed: {e}");
                format!("err {e}\n")
            }
        };
        let _ = send.write_all(resp.as_bytes()).await;
        let _ = send.flush().await;
    }
}

/// Ask a running daemon to reload (used by the mutating commands). Best-effort:
/// if the daemon isn't running (connect fails), this is `Ok(())`. Only an
/// explicit `err` ack from a live daemon is an error.
pub async fn request_reload(state_dir: &Path) -> Result<(), Error> {
    let name = match socket_path(state_dir).to_fs_name::<GenericFilePath>() {
        Ok(n) => n,
        Err(_) => return Ok(()),
    };
    let conn = match LocalSocketStream::connect(name).await {
        Ok(c) => c,
        // No daemon listening — the common case; not an error.
        Err(_) => return Ok(()),
    };
    let (recv, mut send) = conn.split();
    if send.write_all(b"reload\n").await.is_err() {
        return Ok(());
    }
    let _ = send.flush().await;
    let mut lines = BufReader::new(recv).lines();
    match lines.next_line().await {
        Ok(Some(line)) => match line.strip_prefix("err ") {
            Some(msg) => Err(Error::Other(format!("daemon reload failed: {msg}"))),
            None => Ok(()),
        },
        // No / unreadable response — swallow (only an explicit err propagates).
        _ => Ok(()),
    }
}
