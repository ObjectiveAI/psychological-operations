//! Server entrypoints. Other crates can call [`run`] (all-in-one), or split it
//! via [`setup`] + [`serve`] when they need to own the `TcpListener` or wrap
//! the `axum::Router` first.
//!
//! `tag`, `mode`, and the per-session `quota_*` overrides are NOT parameters
//! here. They flow in per-session via the `X-OBJECTIVEAI-ARGUMENTS`
//! JSON-object header — see [`crate::twitch_api::session`] for the
//! source-resolution contract and [`crate::header_session_manager`] for the
//! wiring. State is in-memory only; the manager lazily re-captures the headers
//! from any request landing for a session id it doesn't yet hold, so process
//! restart is transparent to the upstream.

use std::sync::Arc;
use std::time::Duration;

use objectiveai_sdk::cli::command::plugins::run::{Mcp, McpType};
use objectiveai_sdk::cli::plugins::Output;
use psychological_operations_db::Db;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::PsychologicalOperationsTwitchMcp;
use crate::header_session_manager::HeaderSessionManager;
use crate::twitch_api::session::SessionRegistry;

pub async fn setup(
    address: &str,
    port: u16,
    db: Db,
    cache_max_size: u64,
    cache_ttl: Duration,
) -> std::io::Result<(tokio::net::TcpListener, axum::Router)> {
    let registry = Arc::new(SessionRegistry::new());

    let server =
        PsychologicalOperationsTwitchMcp::new(registry.clone(), db, cache_max_size, cache_ttl);

    let session_manager = Arc::new(HeaderSessionManager::new(registry.clone(), server.clone()));
    let ct = CancellationToken::new();

    let service: StreamableHttpService<PsychologicalOperationsTwitchMcp, HeaderSessionManager> =
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
/// Once the listener is bound, emits one JSONL line on **stdout** — the typed
/// [`objectiveai_sdk::cli::plugins::Output::Mcp`] variant carrying the bound
/// URL (`{"type":"mcp","url":"http://127.0.0.1:54321"}`). The host parses it
/// and dials the MCP through the same pipeline a manifest `mcp_servers` entry
/// would. No per-session values in the announcement — clients pin
/// `tag` / `mode` / `quota_*` via the `X-OBJECTIVEAI-ARGUMENTS` header.
pub async fn run(
    address: &str,
    port: u16,
    db: Db,
    cache_max_size: u64,
    cache_ttl: Duration,
) -> std::io::Result<()> {
    let (listener, app) = setup(address, port, db, cache_max_size, cache_ttl).await?;
    let addr = listener.local_addr()?;
    let announcement = Output::Mcp(Mcp {
        r#type: McpType::Mcp,
        url: format!("http://{addr}"),
    });
    println!(
        "{}",
        serde_json::to_string(&announcement).expect("Output::Mcp serializes"),
    );
    serve(listener, app).await
}
