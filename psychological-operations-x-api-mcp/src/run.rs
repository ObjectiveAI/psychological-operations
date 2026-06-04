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

/// All-in-one entrypoint: bind, announce, serve.
///
/// Once the listener is bound, emits one JSONL line on **stdout**
/// announcing the URL — shape matches the
/// `PluginOutput::Notification(value)` wire frame the
/// `psychological-operations-cli` host parses:
///
/// ```jsonc
/// {"value":{"type":"mcp","url":"http://127.0.0.1:54321"}}
/// ```
///
/// The host re-wraps the line in its own
/// `{"type":"notification","value":<this>}` frame. No headers in
/// the announcement — clients pin `(agent, mode)` per session
/// via `X-PSYOP-X-API-{AGENT,MODE}` headers on their initial
/// connect.
pub async fn run(
    address: &str,
    port: u16,
    config_base_dir: PathBuf,
    cache_max_size: u64,
    cache_ttl: Duration,
) -> std::io::Result<()> {
    let (listener, app) = setup(address, port, config_base_dir, cache_max_size, cache_ttl).await?;
    let addr = listener.local_addr()?;
    println!("{}", serde_json::to_string(&serde_json::json!({
        "value": {"type": "mcp", "url": format!("http://{addr}")}
    })).expect("url notification serializes"));
    serve(listener, app).await
}
