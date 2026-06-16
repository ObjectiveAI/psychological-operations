//! Server entrypoints. Other crates can call [`run`] (all-in-one),
//! or split it via [`setup`] + [`serve`] when they need to own the
//! `TcpListener` or wrap the `axum::Router` first.
//!
//! `account`, `mode`, and the per-session `quota_*` overrides are NOT
//! parameters here. They flow in per-session via the
//! `X-OBJECTIVEAI-ARGUMENTS` JSON-object header — see
//! [`crate::x_api::session`] for the source-resolution contract and
//! [`crate::header_session_manager`] for the wiring. State is
//! in-memory only; the manager's `ensure_session` lazily re-captures
//! the headers from any request landing for a session id it doesn't
//! yet hold, so process restart is transparent to the upstream.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use objectiveai_sdk::cli::command::plugins::run::{Mcp, McpType};
use objectiveai_sdk::cli::plugins::Output;
use psychological_operations_db::Db;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::PsychologicalOperationsXApiMcp;
use crate::header_session_manager::HeaderSessionManager;
use crate::x_api::session::SessionRegistry;

pub async fn setup(
    address:         &str,
    port:            u16,
    state_dir:       PathBuf,
    db:              Db,
    cache_max_size:  u64,
    cache_ttl:       Duration,
    mock:            bool,
) -> std::io::Result<(tokio::net::TcpListener, axum::Router)> {
    let registry = Arc::new(SessionRegistry::new());

    let server = PsychologicalOperationsXApiMcp::new(
        registry.clone(),
        reqwest::Client::new(),
        state_dir,
        db,
        cache_max_size,
        cache_ttl,
        mock,
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
/// No per-session values in the announcement — clients pin
/// `account` / `mode` / `quota_*` via the `X-OBJECTIVEAI-ARGUMENTS`
/// header on every request.
pub async fn run(
    address:         &str,
    port:            u16,
    state_dir:       PathBuf,
    db:              Db,
    cache_max_size:  u64,
    cache_ttl:       Duration,
    mock:            bool,
) -> std::io::Result<()> {
    let (listener, app) = setup(
        address, port, state_dir, db, cache_max_size, cache_ttl, mock,
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
