//! Server entrypoints. Other crates can call [`run`] (all-in-one),
//! or split it via [`setup`] + [`serve`] when they need to own the
//! `TcpListener` or wrap the `axum::Router` first.
//!
//! `agent` and `mode` are NOT parameters here. They flow in
//! per-session via the `X-PSYOP-X-API-AGENT` /
//! `X-PSYOP-X-API-MODE` headers — see
//! [`crate::header_session_manager`]. State is in-memory only;
//! the manager's `ensure_session` lazily re-captures headers
//! from any request landing for a session id it doesn't yet
//! hold, so process restart is transparent to the CLI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::PsychologicalOperationsXApiMcp;
use crate::header_session_manager::HeaderSessionManager;
use crate::x_api::session::SessionRegistry;

pub async fn setup(
    address: &str,
    port: u16,
    config_base_dir: PathBuf,
    cache_max_size: u64,
    cache_ttl: Duration,
) -> std::io::Result<(tokio::net::TcpListener, axum::Router)> {
    let registry = Arc::new(SessionRegistry::new());

    let server = PsychologicalOperationsXApiMcp::new(
        registry.clone(),
        reqwest::Client::new(),
        config_base_dir,
        cache_max_size,
        cache_ttl,
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

/// All-in-one entrypoint: bind, announce, serve. Prints
/// `"listening on <addr>"` to stderr once the listener is bound —
/// the CLI supervisor (`mcp/begin.rs::spawn_and_wait`) reads that
/// line to learn the URL.
pub async fn run(
    address: &str,
    port: u16,
    config_base_dir: PathBuf,
    cache_max_size: u64,
    cache_ttl: Duration,
) -> std::io::Result<()> {
    let (listener, app) = setup(address, port, config_base_dir, cache_max_size, cache_ttl).await?;
    let addr = listener.local_addr()?;
    eprintln!("listening on {addr}");
    serve(listener, app).await
}
