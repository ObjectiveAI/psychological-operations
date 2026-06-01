//! Plugin-command dispatch hub.
//!
//! `agents queue handle` works by emitting `TypedPluginOutput::Command`
//! JSON lines on our stdout and reading the host's
//! `PluginCommandResponse` envelopes back on our stdin. The hub here
//! owns the stdin reader, parses each line, and routes the envelope
//! to the per-id mpsc channel registered by [`run`].
//!
//! The host stamps each response with the `id` of the originating
//! command. We mint ids by joining the two queue PK components plus a
//! step suffix (`<operator>::<agent>::message`, `…::list-active`,
//! `…::spawn`) so concurrent agent tasks demux cleanly and a single
//! task's sequential commands stay distinguishable.
//!
//! The hub is lazy-initialized on first call and survives for the
//! lifetime of the process — once stdin is being read here, nothing
//! else in the process can read it.

use std::collections::HashMap;
use std::sync::Arc;

use objectiveai_sdk::cli::output::{Notification, NotificationValue, Output, TypedNotificationValue};
use objectiveai_sdk::cli::plugins::output::{PluginOutput, TypedPluginOutput};
use objectiveai_sdk::cli::plugins::PluginCommandResponse;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, OnceCell, mpsc};

use crate::error::Error;

pub struct DispatchResult {
    pub outputs: Vec<Output>,
    pub exit_code: i32,
}

impl DispatchResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Pull the `agent_id` field out of a `Spawned` notification in
    /// the collected outputs (the `agents spawn` result). Returns
    /// `None` if no `Spawned` notification was seen.
    pub fn spawned_agent_id(&self) -> Option<String> {
        self.outputs.iter().find_map(|out| match out {
            Output::Notification(Notification {
                value: NotificationValue::Typed(TypedNotificationValue::Spawned(s)),
                ..
            }) => Some(s.agent_id.clone()),
            _ => None,
        })
    }

    /// Collect every `ActiveAgent.agent_id` from the outputs. Used
    /// by the `agents list active` step to check whether a stored
    /// handler is still alive.
    pub fn active_agent_ids(&self) -> Vec<String> {
        self.outputs
            .iter()
            .filter_map(|out| match out {
                Output::Notification(Notification {
                    value: NotificationValue::Typed(TypedNotificationValue::ActiveAgent(a)),
                    ..
                }) => Some(a.agent_id.clone()),
                _ => None,
            })
            .collect()
    }
}

type PendingMap = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Output>>>>;

struct Hub {
    pending: PendingMap,
}

static HUB: OnceCell<Arc<Hub>> = OnceCell::const_new();

async fn hub() -> Arc<Hub> {
    HUB.get_or_init(|| async {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(reader_loop(pending.clone()));
        Arc::new(Hub { pending })
    })
    .await
    .clone()
}

async fn reader_loop(pending: PendingMap) {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(resp) = serde_json::from_str::<PluginCommandResponse>(trimmed) else {
            continue;
        };
        let Some(id) = resp.id else { continue };
        let tx = {
            let map = pending.lock().await;
            map.get(&id).cloned()
        };
        if let Some(tx) = tx {
            let is_complete = is_command_complete(&resp.value);
            let _ = tx.send(resp.value);
            if is_complete {
                pending.lock().await.remove(&id);
            }
        }
    }
}

fn is_command_complete(out: &Output) -> bool {
    matches!(
        out,
        Output::Notification(Notification {
            value: NotificationValue::Typed(TypedNotificationValue::CommandComplete(_)),
            ..
        })
    )
}

fn command_complete_exit_code(out: &Output) -> Option<i32> {
    match out {
        Output::Notification(Notification {
            value: NotificationValue::Typed(TypedNotificationValue::CommandComplete(cc)),
            ..
        }) => Some(cc.exit_code),
        _ => None,
    }
}

/// Issue `command` with correlation `id`. Returns when the host
/// emits the matching `CommandComplete` envelope. All preceding
/// outputs ride back in `outputs` (in order).
pub async fn run(id: &str, command: &str) -> Result<DispatchResult, Error> {
    let hub = hub().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    hub.pending.lock().await.insert(id.to_string(), tx);

    let line = serde_json::to_string(&PluginOutput::Typed(TypedPluginOutput::Command {
        id: Some(id.to_string()),
        command: command.to_string(),
    }))
    .map_err(|e| Error::Other(format!("serialize command: {e}")))?;
    println!("{line}");

    let mut outputs: Vec<Output> = Vec::new();
    let mut exit_code = 0i32;
    while let Some(out) = rx.recv().await {
        if let Some(ec) = command_complete_exit_code(&out) {
            exit_code = ec;
            break;
        }
        outputs.push(out);
    }
    hub.pending.lock().await.remove(id);
    Ok(DispatchResult { outputs, exit_code })
}
