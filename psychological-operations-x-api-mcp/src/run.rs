//! Server entrypoints. Other crates can call [`run`] (all-in-one),
//! or split it via [`setup`] + [`serve`] when they need to own the
//! `TcpListener` or wrap the `axum::Router` first.
//!
//! `agent` and `mode` are NOT parameters here. They flow in
//! per-session via the `X-PSYOP-X-API-AGENT` /
//! `X-PSYOP-X-API-MODE` headers — see
//! [`crate::header_session_manager`].
//!
//! On startup, [`setup`] rehydrates every persisted session from
//! the SDK's `x_api_mcp_sessions` SQLite store. For each id we:
//!
//!   1. Spin up a fresh `LocalSessionWorker` via
//!      `create_local_session(id, …)` and wrap it in a
//!      `WorkerTransport`.
//!   2. Spawn a tokio task running `serve_server` over that
//!      transport so the session has a live service end.
//!   3. Replay a synthetic `initialize` request into the worker so
//!      it transitions past its initial state-machine wait and
//!      lands in the main loop ready for real client requests.
//!   4. Insert the `LocalSessionHandle` into the inner
//!      `LocalSessionManager`'s `sessions` map so
//!      `has_session(id)` returns true the moment we start
//!      accepting HTTP requests.
//!
//! When the client reconnects with `Mcp-Session-Id: <id>` and
//! sends initialize, tower's WITH-session-id branch dispatches via
//! `create_stream` — the worker, already past its initial state,
//! processes the second initialize as a normal request through our
//! `ServerHandler::initialize` default impl.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::service::serve_server;
use rmcp::transport::WorkerTransport;
use rmcp::transport::TransportAdapterIdentity;
use rmcp::transport::common::server_side_http::SessionId;
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, SessionConfig, create_local_session,
};
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::PsychologicalOperationsXApiMcp;
use crate::header_session_manager::{HeaderSessionManager, synthetic_initialize_message};
use crate::x_api::session::SessionRegistry;

pub async fn setup(
    address: &str,
    port: u16,
    config_base_dir: PathBuf,
    cache_max_size: u64,
    cache_ttl: Duration,
) -> std::io::Result<(tokio::net::TcpListener, axum::Router)> {
    let registry = Arc::new(
        SessionRegistry::open(&config_base_dir)
            .await
            .map_err(|e| std::io::Error::other(format!("open mcp sessions store: {e}")))?,
    );

    let server = PsychologicalOperationsXApiMcp::new(
        registry.clone(),
        reqwest::Client::new(),
        config_base_dir,
        cache_max_size,
        cache_ttl,
    );

    let session_manager = Arc::new(HeaderSessionManager::new(registry.clone()));

    // Rehydrate every persisted session before exposing the
    // service. Failures are logged and the offending row is
    // dropped — a single bad session never blocks startup.
    for id in registry.ids().await {
        match restore_session(id.clone(), server.clone(), session_manager.inner().clone(), registry.clone())
            .await
        {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(
                    session_id = %id,
                    error = %e,
                    "failed to rehydrate persisted MCP session; dropping",
                );
                let _ = registry.remove(&id).await;
            }
        }
    }

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

/// Bring a single persisted session back to life. See the module
/// doc for the algorithm. Errors here propagate up so `setup`
/// drops the offending row from the store.
async fn restore_session(
    id: SessionId,
    service: PsychologicalOperationsXApiMcp,
    inner: Arc<LocalSessionManager>,
    registry: Arc<SessionRegistry>,
) -> Result<(), String> {
    let (handle, worker) = create_local_session(id.clone(), SessionConfig::default());
    let transport = WorkerTransport::spawn(worker);

    // Spawn the service-side task. Mirrors the tower handler's
    // pattern at `tower.rs:392-416` — when serve_server exits
    // (worker died, transport closed, etc.) we drop the session
    // from both disk and the inner manager's in-memory map.
    {
        let id_for_close = id.clone();
        let registry_for_close = registry.clone();
        let inner_for_close = inner.clone();
        tokio::spawn(async move {
            let serve_result = serve_server::<
                PsychologicalOperationsXApiMcp,
                _,
                _,
                TransportAdapterIdentity,
            >(service, transport)
            .await;
            match serve_result {
                Ok(svc) => {
                    let _ = svc.waiting().await;
                }
                Err(e) => {
                    tracing::warn!(
                        session_id = %id_for_close,
                        error = %e,
                        "serve_server failed for restored session",
                    );
                }
            }
            let _ = registry_for_close.remove(&id_for_close).await;
            inner_for_close.sessions.write().await.remove(&id_for_close);
        });
    }

    // Drive the worker past its `SessionEvent::InitializeRequest`
    // wait state. The response is discarded.
    handle
        .initialize(synthetic_initialize_message())
        .await
        .map_err(|e| format!("synthetic initialize: {e}"))?;

    inner.sessions.write().await.insert(id, handle);
    Ok(())
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

