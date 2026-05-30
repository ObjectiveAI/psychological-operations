use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PsychologicalOperationsRequest {
    #[schemars(description = "The command arguments to pass to the psychological-operations CLI (e.g. [\"psyops\", \"list\"] or [\"reads\", \"run\", \"--name\", \"foo\"])")]
    pub command: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PsychologicalOperationsMcpCli {
    pub tool_router: ToolRouter<Self>,
}

#[tool_router]
impl PsychologicalOperationsMcpCli {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "Psychological Operations CLI",
        description = "Run a psychological-operations CLI command."
    )]
    async fn psychological_operations(
        &self,
        Parameters(req): Parameters<PsychologicalOperationsRequest>,
    ) -> String {
        let args: Vec<String> = std::iter::once("psychological-operations".to_string())
            .chain(req.command)
            .collect();
        let cfg = psychological_operations_cli::load_config();

        // `psychological_operations_cli::run` holds a `&Db` (rusqlite
        // Connection — `!Sync`) across `.await` points, so the future
        // it returns is `!Send`. `rmcp`'s `#[tool]` macro requires
        // `Send` futures so they can be polled on its multi-threaded
        // dispatcher. Bridge: run the !Send future on a dedicated
        // blocking thread under a current-thread runtime that doesn't
        // need Send tasks. spawn_blocking's join handle IS Send, so
        // the outer tool future stays Send.
        //
        // Long-term fix: plumb an `objectiveai_sdk::cli::output::Handle`
        // through `psyops_cli::run` (matching `objectiveai-mcp-cli`'s
        // `Handle::Collect` pattern), which would let the cli emit
        // directly into a `Vec` without holding `&Db` across awaits.
        let join = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build current-thread runtime for psyops cli call");
            rt.block_on(async move {
                psychological_operations_cli::run(args, &cfg).await
            })
        });

        match join.await {
            Ok(Ok(output)) => output,
            Ok(Err(e))     => format!("error: {e}"),
            Err(e)         => format!("error: cli task panicked: {e}"),
        }
    }
}

#[tool_handler]
impl ServerHandler for PsychologicalOperationsMcpCli {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "psychological-operations-cli".into(),
                title: None,
                version: env!("CARGO_PKG_VERSION").into(),
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }
}
