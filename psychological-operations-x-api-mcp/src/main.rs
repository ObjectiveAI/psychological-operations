use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use objectiveai_sdk::cli::command::binary::BinaryExecutor;

/// X-API MCP server. Drives a streamable-HTTP MCP that proxies the X
/// v2 API, intermediated by the postgres-backed response cache and the
/// two-tier (in-process + advisory) lock in the db crate.
///
/// `agent` and `mode` are NOT flags — clients supply them per
/// session via the `X-OBJECTIVEAI-ARGUMENTS` JSON-object header
/// (and `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` as the agent
/// fallback) on every connect. See `crate::x_api::session` for
/// the source-resolution contract.
#[derive(Parser)]
#[command(name = "psychological-operations-x-api-mcp")]
struct Args {
    /// Root of the remaining on-disk state — the CEF profile tree
    /// (cookie probe) lives under it (the `OBJECTIVEAI_STATE_DIR`
    /// value). Assumed to already exist.
    #[arg(long)]
    state_dir: PathBuf,
    /// Postgres connection URL — the single persistence layer (the
    /// `OBJECTIVEAI_POSTGRES_URL` value).
    #[arg(long, env = "OBJECTIVEAI_POSTGRES_URL")]
    postgres_url: String,
    /// Cache budget in bytes.
    #[arg(long)]
    cache_max_size: u64,
    /// Per-entry cache TTL in seconds.
    #[arg(long)]
    cache_ttl: u64,
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
    let executor = BinaryExecutor::new(Some(args.state_dir.clone()));
    let db = psychological_operations_db::Db::connect(&args.postgres_url)
        .await
        .map_err(|e| std::io::Error::other(format!("db connect: {e}")))?;
    psychological_operations_x_api_mcp::run(
        &args.address,
        args.port,
        args.state_dir,
        db,
        args.cache_max_size,
        Duration::from_secs(args.cache_ttl),
        executor,
    )
    .await
}
