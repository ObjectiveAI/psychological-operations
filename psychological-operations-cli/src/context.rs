//! Per-process singleton bundle. One [`Context`] is built once in
//! `main.rs` (via [`Context::new`]) and threaded as `&Context`
//! through every handler. Holds two things every handler needs:
//!
//! - `config` — the env-derived runtime knobs
//!   ([`crate::run::Config`]) that handlers used to read directly
//!   as a `cfg: &crate::run::Config` arg.
//! - `executor` — the SDK's in-process [`PluginExecutor`],
//!   `Arc`-wrapped so handlers that fan it out into nested tasks
//!   (e.g. `agents queue handle`'s per-agent `JoinSet`) can
//!   `clone()` cheaply. `PluginExecutor::new()` consumes process
//!   stdin/stdout, so there can only ever be one per process —
//!   building `Context` exactly once in `main` enforces that
//!   invariant.

use std::sync::Arc;

use objectiveai_sdk::cli::command::plugin::PluginExecutor;

pub struct Context {
    pub config:   crate::run::Config,
    pub executor: Arc<PluginExecutor>,
}

impl Context {
    /// Build a `Context` from the process environment: load the
    /// runtime config via [`crate::run::load_config`] and
    /// construct the SDK's single-instance [`PluginExecutor`].
    pub fn new() -> Self {
        Self {
            config:   crate::run::load_config(),
            executor: Arc::new(PluginExecutor::new()),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}
