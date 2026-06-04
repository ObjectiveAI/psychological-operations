//! Lazy global `PluginExecutor` accessor.
//!
//! The SDK's `PluginExecutor` consumes the process's
//! `tokio::io::stdin()` / `tokio::io::stdout()` handles in its
//! constructor and spawns its own demux task — so there can only
//! be one instance per process. We lazy-init it on the first
//! caller. Every objectiveai-CLI in-process call in this crate
//! routes through `objectiveai_executor::executor().await`.

use std::sync::Arc;

use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use tokio::sync::OnceCell;

static EXECUTOR: OnceCell<Arc<PluginExecutor>> = OnceCell::const_new();

pub async fn executor() -> Arc<PluginExecutor> {
    EXECUTOR
        .get_or_init(|| async { Arc::new(PluginExecutor::new()) })
        .await
        .clone()
}
