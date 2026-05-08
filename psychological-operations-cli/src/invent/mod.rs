use clap::{Args, Subcommand};
use objectiveai::functions::inventions::{
    ParamsState,
    state::{AlphaScalarState, AlphaVectorState, Params},
};

use crate::input;

#[derive(Args)]
pub struct InventionParams {
    /// Function name
    #[arg(long)]
    pub name: String,
    /// Specification/prompt for the invention
    #[arg(long)]
    pub spec: String,
    /// Nesting depth (0 for leaf-only)
    #[arg(long, default_value = "0")]
    pub depth: u64,
    /// Minimum branch width
    #[arg(long, default_value = "2")]
    pub min_branch_width: u64,
    /// Maximum branch width
    #[arg(long, default_value = "3")]
    pub max_branch_width: u64,
    /// Minimum leaf width (tasks per leaf)
    #[arg(long, default_value = "1")]
    pub min_leaf_width: u64,
    /// Maximum leaf width (tasks per leaf)
    #[arg(long, default_value = "5")]
    pub max_leaf_width: u64,
}

impl InventionParams {
    fn into_params(self) -> Params {
        Params {
            depth: self.depth,
            min_branch_width: self.min_branch_width,
            max_branch_width: self.max_branch_width,
            min_leaf_width: self.min_leaf_width,
            max_leaf_width: self.max_leaf_width,
            name: self.name,
            spec: self.spec,
        }
    }
}

/// Args forwarded verbatim to the objectiveai CLI.
#[derive(Args)]
pub struct ForwardArgs {
    /// Agent reference (e.g. favorite=name or remote=github,owner=x,...)
    #[arg(long)]
    agent: Option<String>,
    /// Inline JSON agent definition
    #[arg(long)]
    agent_inline: Option<String>,
    /// ID from the matching `instructions get` subcommand. Required by
    /// objectiveai's recursive create.
    #[arg(long)]
    instructions_id: String,
    /// Seed for deterministic mock responses
    #[arg(long)]
    seed: Option<i64>,
    /// Run in the background: print PID and log path, then exit
    #[arg(long)]
    detach: bool,
    /// OpenRouter continuation from a previous response (base64-encoded)
    #[arg(long)]
    openrouter_continuation_from_response: Option<String>,
    /// Claude Agent SDK continuation from a previous response (base64-encoded)
    #[arg(long)]
    claude_agent_sdk_continuation_from_response: Option<String>,
    /// Mock continuation from a previous response (base64-encoded)
    #[arg(long)]
    mock_continuation_from_response: Option<String>,
    /// OpenRouter continuation messages as inline JSON
    #[arg(long)]
    openrouter_continuation_messages_inline: Option<String>,
    /// OpenRouter continuation messages from inline Python code
    #[arg(long)]
    openrouter_continuation_messages_python_inline: Option<String>,
    /// OpenRouter continuation messages from a Python file
    #[arg(long)]
    openrouter_continuation_messages_python_file: Option<std::path::PathBuf>,
    /// Mock continuation messages as inline JSON
    #[arg(long)]
    mock_continuation_messages_inline: Option<String>,
    /// Mock continuation messages from inline Python code
    #[arg(long)]
    mock_continuation_messages_python_inline: Option<String>,
    /// Mock continuation messages from a Python file
    #[arg(long)]
    mock_continuation_messages_python_file: Option<std::path::PathBuf>,
    /// Claude Agent SDK continuation with a session ID
    #[arg(long)]
    claude_agent_sdk_continuation_session_id: Option<String>,
}

impl ForwardArgs {
    fn append_to(&self, args: &mut Vec<String>) {
        if let Some(ref v) = self.agent {
            args.extend(["--agent".into(), v.clone()]);
        }
        if let Some(ref v) = self.agent_inline {
            args.extend(["--agent-inline".into(), v.clone()]);
        }
        args.extend(["--instructions-id".into(), self.instructions_id.clone()]);
        if let Some(v) = self.seed {
            args.extend(["--seed".into(), v.to_string()]);
        }
        if self.detach {
            args.push("--detach".into());
        }
        if let Some(ref v) = self.openrouter_continuation_from_response {
            args.extend(["--openrouter-continuation-from-response".into(), v.clone()]);
        }
        if let Some(ref v) = self.claude_agent_sdk_continuation_from_response {
            args.extend(["--claude-agent-sdk-continuation-from-response".into(), v.clone()]);
        }
        if let Some(ref v) = self.mock_continuation_from_response {
            args.extend(["--mock-continuation-from-response".into(), v.clone()]);
        }
        if let Some(ref v) = self.openrouter_continuation_messages_inline {
            args.extend(["--openrouter-continuation-messages-inline".into(), v.clone()]);
        }
        if let Some(ref v) = self.openrouter_continuation_messages_python_inline {
            args.extend(["--openrouter-continuation-messages-python-inline".into(), v.clone()]);
        }
        if let Some(ref v) = self.openrouter_continuation_messages_python_file {
            args.extend(["--openrouter-continuation-messages-python-file".into(), v.display().to_string()]);
        }
        if let Some(ref v) = self.mock_continuation_messages_inline {
            args.extend(["--mock-continuation-messages-inline".into(), v.clone()]);
        }
        if let Some(ref v) = self.mock_continuation_messages_python_inline {
            args.extend(["--mock-continuation-messages-python-inline".into(), v.clone()]);
        }
        if let Some(ref v) = self.mock_continuation_messages_python_file {
            args.extend(["--mock-continuation-messages-python-file".into(), v.display().to_string()]);
        }
        if let Some(ref v) = self.claude_agent_sdk_continuation_session_id {
            args.extend(["--claude-agent-sdk-continuation-session-id".into(), v.clone()]);
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Invent a scalar function for scoring individual posts
    AlphaScalar {
        #[command(flatten)]
        params: InventionParams,
        #[command(flatten)]
        forward: ForwardArgs,
    },
    /// Invent a vector function for ranking posts
    AlphaVector {
        #[command(flatten)]
        params: InventionParams,
        #[command(flatten)]
        forward: ForwardArgs,
    },
    /// Invent from existing state (remote reference or inline JSON)
    Remote {
        /// State reference (e.g. remote=mock,name=inv-good-sl)
        #[arg(long, required_unless_present = "state_inline")]
        state: Option<String>,
        /// Inline JSON state
        #[arg(long, conflicts_with = "state")]
        state_inline: Option<String>,
        #[command(flatten)]
        forward: ForwardArgs,
    },
}

impl Commands {
    pub fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::AlphaScalar { params, forward } => {
                let p = params.into_params();
                let state = ParamsState::AlphaScalar(AlphaScalarState {
                    params: p,
                    input_schema: Some(input::scalar_input_schema()),
                });
                run_invention(&state, &forward, cfg)
            }
            Commands::AlphaVector { params, forward } => {
                let p = params.into_params();
                let state = ParamsState::AlphaVector(AlphaVectorState {
                    params: p,
                    input_schema: Some(input::vector_input_schema()),
                });
                run_invention(&state, &forward, cfg)
            }
            Commands::Remote { state, state_inline, forward } => {
                let resolved = if let Some(inline) = state_inline {
                    let parsed: ParamsState = serde_json::from_str(&inline)?;
                    fill_schema_if_missing(parsed)
                } else if let Some(ref ref_str) = state {
                    let fetched = fetch_state(ref_str, cfg)?;
                    fill_schema_if_missing(fetched)
                } else {
                    return Err(crate::error::Error::Other("--state or --state-inline is required".into()));
                };
                run_invention(&resolved, &forward, cfg)
            }
        }
    }
}

/// Fetch a remote invention state via the CLI.
fn fetch_state(ref_str: &str, cfg: &crate::run::Config) -> Result<ParamsState, crate::error::Error> {
    let output = std::process::Command::new(crate::score::objectiveai_binary(cfg))
        .args(["functions", "inventions", "state", "get", "--path", ref_str])
        .stderr(std::process::Stdio::inherit())
        .output()?;

    if !output.status.success() {
        return Err(crate::error::Error::ObjectiveAiCli("failed to fetch invention state".into()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let state: ParamsState = serde_json::from_str(stdout.trim())?;
    Ok(state)
}

/// Fill input_schema if it's missing, using our post schema.
fn fill_schema_if_missing(state: ParamsState) -> ParamsState {
    match state {
        ParamsState::AlphaScalar(mut s) => {
            if s.input_schema.is_none() {
                s.input_schema = Some(input::scalar_input_schema());
            }
            ParamsState::AlphaScalar(s)
        }
        ParamsState::AlphaVector(mut s) => {
            if s.input_schema.is_none() {
                s.input_schema = Some(input::vector_input_schema());
            }
            ParamsState::AlphaVector(s)
        }
        ParamsState::AlphaScalarBranch(mut s) => {
            if s.input_schema.is_none() {
                s.input_schema = Some(input::scalar_input_schema());
            }
            ParamsState::AlphaScalarBranch(s)
        }
        ParamsState::AlphaScalarLeaf(mut s) => {
            if s.input_schema.is_none() {
                s.input_schema = Some(input::scalar_input_schema());
            }
            ParamsState::AlphaScalarLeaf(s)
        }
        ParamsState::AlphaVectorBranch(mut s) => {
            if s.input_schema.is_none() {
                s.input_schema = Some(input::vector_input_schema());
            }
            ParamsState::AlphaVectorBranch(s)
        }
        ParamsState::AlphaVectorLeaf(mut s) => {
            if s.input_schema.is_none() {
                s.input_schema = Some(input::vector_input_schema());
            }
            ParamsState::AlphaVectorLeaf(s)
        }
    }
}

/// Shell out to objectiveai CLI via `remote --state-inline`.
fn run_invention(
    state: &ParamsState,
    fwd: &ForwardArgs,
    cfg: &crate::run::Config,
) -> Result<crate::Output, crate::error::Error> {
    let state_json = serde_json::to_string(state)?;

    let mut args = vec![
        "functions".into(),
        "inventions".into(),
        "recursive".into(),
        "create".into(),
        "remote".into(),
        "--state-inline".into(),
        state_json,
    ];

    fwd.append_to(&mut args);

    let status = std::process::Command::new(crate::score::objectiveai_binary(cfg))
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(crate::error::Error::ObjectiveAiCli("invention failed".into()));
    }

    Ok(crate::Output::Empty)
}
