//! Per-process singleton bundle. One [`Context`] is built once in
//! `main.rs` (via [`Context::new`]) and threaded as `&Context`
//! through every handler. Holds two things every handler needs:
//!
//! - `config` — the env-derived runtime knobs
//!   ([`crate::run::Config`]) that handlers used to read directly
//!   as a `cfg: &crate::run::Config` arg.
//! - `executor` — the SDK's in-process [`PluginExecutor`],
//!   `Arc`-wrapped so handlers that fan it out into nested tasks
//!   can `clone()` cheaply. `PluginExecutor::new()` consumes process
//!   stdin/stdout, so there can only ever be one per process —
//!   building `Context` exactly once in `main` enforces that
//!   invariant.

use std::sync::Arc;
use std::time::Duration;

use objectiveai_sdk::cli::command::plugin::PluginExecutor;
use psychological_operations_db::Db;

/// Default budget for the response cache (the `cache` table in
/// postgres). 2 GiB.
const DEFAULT_CACHE_MAX_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Default per-entry TTL for the SDK Client's response cache.
/// Plumbed today, consumed by future time-based eviction.
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

pub struct Context {
    pub config: crate::run::Config,
    pub executor: Arc<PluginExecutor>,
    /// The single persistence layer (postgres pool). Cloned into every
    /// `x::client::Client` and used directly by command handlers for
    /// psyops/config/posts/queue access. Built once in [`Context::new`].
    pub db: Db,
    /// Bytes. Passed to `x::client::Client::new` as the response
    /// cache's size budget. One source of truth across every CLI Client
    /// construction site.
    pub cache_max_size: u64,
    /// Per-entry TTL passed to `x::client::Client::new`. Same
    /// rationale as `cache_max_size` — single source of truth.
    pub cache_ttl: Duration,
}

impl Context {
    /// Build a `Context` from the process environment: load the runtime
    /// config via [`crate::run::load_config`], connect the postgres
    /// persistence layer ([`Db::connect`], which applies the schema),
    /// and construct the SDK's single-instance [`PluginExecutor`].
    ///
    /// Async + fallible: opening the pool can fail (bad URL, server
    /// down). Callers in `main` propagate the error.
    pub async fn new() -> Result<Self, crate::error::Error> {
        let config = crate::run::load_config();
        let db = Db::connect(&config.postgres_url)
            .await
            .map_err(|e| crate::error::Error::Other(format!("db connect: {e}")))?;
        let ctx = Self {
            config,
            executor: Arc::new(PluginExecutor::new()),
            db,
            cache_max_size: DEFAULT_CACHE_MAX_SIZE,
            cache_ttl: DEFAULT_CACHE_TTL,
        };
        Ok(ctx)
    }
}
