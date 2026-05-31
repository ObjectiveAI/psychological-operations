//! Psychological Operations X-API MCP server.
//!
//! Mirrors the `psychological-operations-mcp` `run.rs` shape so other crates can
//! `use psychological_operations_x_api_mcp::{ConfigBuilder, run}` and spawn the
//! server in-process without going through the binary.

use std::path::PathBuf;
use std::sync::Arc;

use envconfig::Envconfig;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

use crate::x_api::PsychologicalOperationsXApiMcp;

#[derive(Envconfig)]
struct EnvConfigBuilder {
    #[envconfig(from = "ADDRESS")]              address: Option<String>,
    #[envconfig(from = "PORT")]                 port: Option<u16>,
    #[envconfig(from = "SUPPRESS_OUTPUT")]      suppress_output: Option<String>,
    #[envconfig(from = "CONFIG_BASE_DIR")]      config_base_dir: Option<String>,
    #[envconfig(from = "MAX_CACHE_SIZE")]       max_cache_size: Option<u64>,
    #[envconfig(from = "CACHE_TTL_SECS")]       cache_ttl_secs: Option<u64>,
}

impl EnvConfigBuilder {
    fn build(self) -> ConfigBuilder {
        ConfigBuilder {
            address: self.address,
            port: self.port,
            suppress_output: self.suppress_output.map(|v| {
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
            }),
            config_base_dir: self.config_base_dir,
            max_cache_size: self.max_cache_size,
            cache_ttl_secs: self.cache_ttl_secs,
        }
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    pub address:          Option<String>,
    pub port:             Option<u16>,
    pub suppress_output:  Option<bool>,
    pub config_base_dir:  Option<String>,
    pub max_cache_size:   Option<u64>,
    pub cache_ttl_secs:   Option<u64>,
}

impl Envconfig for ConfigBuilder {
    #[allow(deprecated)]
    fn init() -> Result<Self, envconfig::Error> {
        EnvConfigBuilder::init().map(|e| e.build())
    }

    fn init_from_env() -> Result<Self, envconfig::Error> {
        EnvConfigBuilder::init_from_env().map(|e| e.build())
    }

    fn init_from_hashmap(
        hashmap: &std::collections::HashMap<String, String>,
    ) -> Result<Self, envconfig::Error> {
        EnvConfigBuilder::init_from_hashmap(hashmap).map(|e| e.build())
    }
}

impl ConfigBuilder {
    pub fn build(self) -> Config {
        Config {
            address: self.address.unwrap_or_else(|| "0.0.0.0".to_string()),
            port:    self.port.unwrap_or(3001),
            suppress_output: self.suppress_output.unwrap_or(false),
            config_base_dir: self
                .config_base_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".objectiveai")
                }),
            max_cache_size: self.max_cache_size.unwrap_or(256 * 1024 * 1024),
            cache_ttl_secs: self.cache_ttl_secs.unwrap_or(3600),
        }
    }
}

pub struct Config {
    pub address:          String,
    pub port:             u16,
    pub suppress_output:  bool,
    /// Outer root for the x-api cache + `x_app.json`. Same shape the SDK's
    /// `auth_json` / `Client::new` expect. Default `~/.objectiveai`.
    pub config_base_dir:  PathBuf,
    /// Bytes — x-api response-cache size budget (`Client::new`'s
    /// `cache_max_size`). Default 256 MB.
    pub max_cache_size:   u64,
    /// Seconds — per-entry cache TTL (`Client::new`'s `cache_ttl`).
    /// Currently plumbed but unused. Default 3600 (1 h).
    pub cache_ttl_secs:   u64,
}

pub async fn setup(config: Config) -> std::io::Result<(tokio::net::TcpListener, axum::Router)> {
    let Config {
        address,
        port,
        suppress_output: _,
        config_base_dir,
        max_cache_size,
        cache_ttl_secs,
    } = config;

    let http = Client::new(
        reqwest::Client::new(),
        /* mock */ false,
        max_cache_size,
        std::time::Duration::from_secs(cache_ttl_secs),
        config_base_dir,
        AuthMode::XApp,
    );

    let server = PsychologicalOperationsXApiMcp::new(Arc::new(http));
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

pub async fn run(config: Config) -> std::io::Result<()> {
    let suppress_output = config.suppress_output;
    let (listener, app) = setup(config).await?;
    if !suppress_output {
        let addr = listener.local_addr()?;
        eprintln!("listening on {addr}");
    }
    serve(listener, app).await
}
