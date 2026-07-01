use std::time::Duration;

use clap::Parser;

/// Twitch MCP server. Drives a streamable-HTTP MCP backed by the
/// postgres-stored Twitch chat buffer + channel-join set (reads) and the
/// Helix-backed Twitch client (writes).
///
/// `tag`, `mode`, and the per-session `quota_*` overrides are NOT flags —
/// clients supply them per session via the `X-OBJECTIVEAI-ARGUMENTS`
/// JSON-object header on every connect. See `crate::twitch_api::session` for
/// the source-resolution contract.
#[derive(Parser)]
#[command(name = "psychological-operations-twitch-mcp")]
struct Args {
    /// Postgres connection URL — the single persistence layer (the
    /// `OBJECTIVEAI_POSTGRES_URL` value).
    #[arg(long, env = "OBJECTIVEAI_POSTGRES_URL")]
    postgres_url: String,
    /// Response-cache budget in bytes.
    #[arg(long)]
    cache_max_size: u64,
    /// Per-entry response-cache TTL in seconds.
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
    let db = psychological_operations_db::Db::connect(&args.postgres_url)
        .await
        .map_err(|e| std::io::Error::other(format!("db connect: {e}")))?;
    psychological_operations_twitch_mcp::run(
        &args.address,
        args.port,
        db,
        args.cache_max_size,
        Duration::from_secs(args.cache_ttl),
    )
    .await
}
