//! Server entrypoints. Other crates can call [`run`] (all-in-one),
//! or split it via [`setup`] + [`serve`] when they need to own the
//! `TcpListener` or wrap the `axum::Router` first.
//!
//! `agent` and `mode` are NOT parameters here. They flow in
//! per-session via the `X-OBJECTIVEAI-ARGUMENTS` JSON-object
//! header (with `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the
//! agent fallback) — see [`crate::x_api::session`] for the
//! source-resolution contract and
//! [`crate::header_session_manager`] for the wiring. State is
//! in-memory only; the manager's `ensure_session` lazily
//! re-captures the headers from any request landing for a
//! session id it doesn't yet hold, so process restart is
//! transparent to the upstream.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use objectiveai_sdk::cli::command::CommandExecutor;
use objectiveai_sdk::cli::command::plugins::run::{Mcp, McpType};
use objectiveai_sdk::cli::plugins::Output;
use psychological_operations_db::Db;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::PsychologicalOperationsXApiMcp;
use crate::header_session_manager::HeaderSessionManager;
use crate::x_api::accounts::AgentTagLister;
use crate::x_api::session::SessionRegistry;

pub async fn setup<E>(
    address:         &str,
    port:            u16,
    state_dir:       PathBuf,
    db:              Db,
    cache_max_size:  u64,
    cache_ttl:       Duration,
    executor:        E,
) -> std::io::Result<(tokio::net::TcpListener, axum::Router)>
where
    E: CommandExecutor + Send + Sync + 'static,
    E::Error: std::fmt::Display + Send + 'static,
{
    // The executor backs `list_accounts` (it runs `agents instances get`
    // on the session's AIH to discover bound tags). The generic executor
    // isn't object-safe, so box it behind the object-safe `AgentTagLister`
    // shim (blanket-impl'd for every executor) before handing it to the
    // concrete server type.
    let accounts: Arc<dyn AgentTagLister> = Arc::new(executor);

    let registry = Arc::new(SessionRegistry::new());

    let server = PsychologicalOperationsXApiMcp::new(
        registry.clone(),
        reqwest::Client::new(),
        state_dir,
        db,
        cache_max_size,
        cache_ttl,
        accounts,
    );

    let session_manager = Arc::new(HeaderSessionManager::new(registry.clone(), server.clone()));
    let ct = CancellationToken::new();

    let service: StreamableHttpService<PsychologicalOperationsXApiMcp, HeaderSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            session_manager,
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: None,
                cancellation_token: ct.child_token(),
                ..Default::default()
            },
        );

    let router = axum::Router::new().fallback_service(service);
    let listener = tokio::net::TcpListener::bind(format!("{address}:{port}")).await?;
    Ok((listener, router))
}

pub async fn serve(listener: tokio::net::TcpListener, app: axum::Router) -> std::io::Result<()> {
    axum::serve(listener, app).await
}

/// All-in-one entrypoint: bind, announce, serve.
///
/// Once the listener is bound, emits one JSONL line on **stdout**
/// — the typed [`objectiveai_sdk::cli::plugins::Output::Mcp`]
/// variant carrying the bound URL:
///
/// ```jsonc
/// {"type":"mcp","url":"http://127.0.0.1:54321"}
/// ```
///
/// The host (running this crate either as `mcp begin` from the
/// CLI or as a standalone binary under the objectiveai
/// supervisor) parses the line as
/// `cli::plugins::Output::Mcp(Mcp { … })` and routes the URL
/// through the same "dial this MCP" pipeline a manifest
/// `mcp_servers` entry would — see the docstring on
/// [`objectiveai_sdk::cli::command::plugins::run::Mcp`].
///
/// No `(agent, mode)` in the announcement — clients pin those
/// per session via the `X-OBJECTIVEAI-ARGUMENTS` header (with
/// `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the agent
/// fallback) on every request.
pub async fn run<E>(
    address:         &str,
    port:            u16,
    state_dir:       PathBuf,
    db:              Db,
    cache_max_size:  u64,
    cache_ttl:       Duration,
    executor:        E,
) -> std::io::Result<()>
where
    E: CommandExecutor + Send + Sync + 'static,
    E::Error: std::fmt::Display + Send + 'static,
{
    let (listener, app) = setup(
        address, port, state_dir, db, cache_max_size, cache_ttl, executor,
    ).await?;
    let addr = listener.local_addr()?;
    let announcement = Output::Mcp(Mcp {
        r#type: McpType::Mcp,
        url:    format!("http://{addr}"),
    });
    println!(
        "{}",
        serde_json::to_string(&announcement).expect("Output::Mcp serializes"),
    );
    serve(listener, app).await
}
