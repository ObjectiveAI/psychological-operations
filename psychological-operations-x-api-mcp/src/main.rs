use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_x_api_mcp::{Mode, PsychologicalOperationsXApiMcp};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

/// X-API MCP server. Drives a streamable-HTTP MCP that proxies the X
/// v2 API, intermediated by a sqlx-backed response cache and a
/// two-tier (in-process + cross-process) lock.
#[derive(Parser)]
#[command(name = "psychological-operations-x-api-mcp")]
struct Args {
    /// Outer root for the x-api cache + `x_app.json`. Same shape the
    /// SDK's `auth_json` / `Client::new` expect.
    #[arg(long)]
    config_base_dir: PathBuf,
    /// Cache budget in bytes.
    #[arg(long)]
    cache_max_size: u64,
    /// Per-entry cache TTL in seconds.
    #[arg(long)]
    cache_ttl: u64,
    /// Agent whose persona OAuth token authenticates every X API
    /// call.
    #[arg(long)]
    agent: String,
    /// Tool-surface mode. `readonly` exposes only read tools;
    /// `full` adds the mutating tools (post / reply / quote / like /
    /// retweet / bookmark).
    #[arg(long, value_enum)]
    mode: Mode,
    /// Bind address — hidden; supervisor-internal.
    #[arg(long, default_value = "127.0.0.1", hide = true)]
    address: String,
    /// Bind port (0 = OS picks) — hidden; supervisor-internal.
    #[arg(long, default_value_t = 0, hide = true)]
    port: u16,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::DEBUG.into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args = Args::parse();

    let http = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        args.cache_max_size,
        Duration::from_secs(args.cache_ttl),
        args.config_base_dir,
        AuthMode::Agent(args.agent.clone()),
    );

    let server = PsychologicalOperationsXApiMcp::new(Arc::new(http), args.mode, args.agent);
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
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", args.address, args.port)).await?;
    let addr = listener.local_addr()?;
    eprintln!("listening on {addr}");

    axum::serve(listener, router).await
}
