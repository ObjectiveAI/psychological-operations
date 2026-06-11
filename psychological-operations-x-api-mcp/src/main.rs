use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use objectiveai_sdk::cli::command::binary::BinaryExecutor;

/// X-API MCP server. Drives a streamable-HTTP MCP that proxies the X
/// v2 API, intermediated by a sqlx-backed response cache and a
/// two-tier (in-process + cross-process) lock.
///
/// `agent` and `mode` are NOT flags — clients supply them per
/// session via the `X-OBJECTIVEAI-ARGUMENTS` JSON-object header
/// (and `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the agent
/// fallback) on every connect. See `crate::x_api::session` for
/// the source-resolution contract.
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
    /// Read quota: max GET X-API requests per caller (agent
    /// instance hierarchy) per trailing hour.
    #[arg(long, default_value_t = 50)]
    quota_read: u64,
    /// Write quota: max non-GET (POST/PUT/DELETE/PATCH) X-API
    /// requests per caller per trailing hour.
    #[arg(long, default_value_t = 10)]
    quota_write: u64,
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
    let executor = BinaryExecutor::new(Some(args.config_base_dir.clone()));
    psychological_operations_x_api_mcp::run(
        &args.address,
        args.port,
        args.config_base_dir,
        args.cache_max_size,
        Duration::from_secs(args.cache_ttl),
        args.quota_read,
        args.quota_write,
        executor,
    )
    .await
}
