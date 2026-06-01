use clap::Parser;
use envconfig::Envconfig;

use psychological_operations_x_api_mcp::Mode;

/// X-API MCP server. Drives a streamable-HTTP MCP that proxies the X
/// v2 API, intermediated by a sqlx-backed response cache and a
/// two-tier (in-process + cross-process) lock.
#[derive(Parser)]
#[command(name = "psychological-operations-x-api-mcp")]
struct Args {
    /// Cache budget in bytes.
    #[arg(long)]
    cache_max_size: u64,
    /// Per-entry cache TTL in seconds.
    #[arg(long)]
    cache_ttl: u64,
    /// Agent whose persona OAuth token authenticates every X API
    /// call. Required — no env fallback.
    #[arg(long)]
    agent: String,
    /// Operator lineage identity (`OBJECTIVEAI_AGENT_ID` upstream).
    /// Partitions the per-agent queue so different operators sharing
    /// the same cache file don't see each other's rows. Required —
    /// the CLI supervisor sources it from `Config.objectiveai_agent_id`.
    #[arg(long)]
    objectiveai_agent_id: String,
    /// Tool-surface mode. `readonly` exposes only read tools;
    /// `full` adds the mutating tools (post / reply / quote / like /
    /// retweet / bookmark). Required — no default at the binary
    /// level; defaulting is the CLI supervisor's job.
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

    let _ = dotenv::dotenv();
    let args = Args::parse();

    // env-default chain still wins for suppress_output + config_base_dir;
    // clap overrides win for the five flag-driven fields + agent + mode.
    let mut builder = psychological_operations_x_api_mcp::ConfigBuilder::init_from_env()
        .unwrap_or_default();
    builder.address = Some(args.address);
    builder.port = Some(args.port);
    builder.max_cache_size = Some(args.cache_max_size);
    builder.cache_ttl_secs = Some(args.cache_ttl);
    builder.agent = Some(args.agent);
    builder.objectiveai_agent_id = Some(args.objectiveai_agent_id);
    builder.mode = Some(args.mode);

    psychological_operations_x_api_mcp::run(builder.build()).await
}
