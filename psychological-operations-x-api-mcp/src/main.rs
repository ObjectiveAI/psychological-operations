use clap::Parser;
use envconfig::Envconfig;

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
    /// Agent. Falls back to `OBJECTIVEAI_AGENT_ID_BASE` env if absent;
    /// fatal error if neither is set.
    #[arg(long)]
    agent: Option<String>,
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
    let agent = args
        .agent
        .or_else(|| std::env::var("OBJECTIVEAI_AGENT_ID_BASE").ok())
        .ok_or_else(|| {
            std::io::Error::other(
                "agent must be specified via --agent or OBJECTIVEAI_AGENT_ID_BASE",
            )
        })?;

    // env-default chain still wins for suppress_output + config_base_dir;
    // clap overrides win for the four flag-driven fields + agent.
    let mut builder = psychological_operations_x_api_mcp::ConfigBuilder::init_from_env()
        .unwrap_or_default();
    builder.address = Some(args.address);
    builder.port = Some(args.port);
    builder.max_cache_size = Some(args.cache_max_size);
    builder.cache_ttl_secs = Some(args.cache_ttl);
    builder.objectiveai_agent_id_base = Some(agent);

    psychological_operations_x_api_mcp::run(builder.build()).await
}
