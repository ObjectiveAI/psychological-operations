//! Server entrypoints. Other crates can call [`run`] (all-in-one),
//! or split it via [`setup`] + [`serve`] when they need to own the
//! `TcpListener` or wrap the `axum::Router` first.
//!
//! All parameters are explicit; there is no `Config` struct and no
//! env-var layer. The binary's clap args (`main.rs`) are the sole
//! source of truth for the values these functions receive.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use psychological_operations_sdk::x::client::{AuthMode, Client};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

use crate::Mode;
use crate::x_api::PsychologicalOperationsXApiMcp;

pub async fn setup(
    address: &str,
    port: u16,
    config_base_dir: PathBuf,
    cache_max_size: u64,
    cache_ttl: Duration,
    agent: String,
    mode: Mode,
) -> std::io::Result<(tokio::net::TcpListener, axum::Router)> {
    let http = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        cache_max_size,
        cache_ttl,
        config_base_dir,
        AuthMode::Agent(agent.clone()),
    );

    let server = PsychologicalOperationsXApiMcp::new(Arc::new(http), mode, agent);
    let ct = CancellationToken::new();

    let service: StreamableHttpService<PsychologicalOperationsXApiMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
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
    agent: String,
    mode: Mode,
) -> std::io::Result<()> {
    let (listener, app) = setup(
        address,
        port,
        config_base_dir,
        cache_max_size,
        cache_ttl,
        agent,
        mode,
    )
    .await?;
    let addr = listener.local_addr()?;
    eprintln!("listening on {addr}");
    serve(listener, app).await
}
