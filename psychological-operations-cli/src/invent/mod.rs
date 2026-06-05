use clap::Args;
use futures::StreamExt;
use objectiveai_sdk::cli::command::agents::spawn::AgentSpec;
use objectiveai_sdk::cli::command::functions::inventions::{
    recursive::create::{
        Request as RecursiveCreateRequest,
        ResponseItem as RecursiveResponseItem,
        remote as recursive_remote,
    },
    state::get as state_get,
};
use objectiveai_sdk::functions::inventions::{
    ParamsState,
    state::Params,
};
use psychological_operations_sdk::cli::Output;

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
    pub(crate) fn into_params(self) -> Params {
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

/// Args forwarded verbatim to the recursive-invent executor.
#[derive(Args)]
pub struct ForwardArgs {
    /// Inline JSON agent definition (preferred). The recursive
    /// executor demands the fully resolved agent value at request
    /// time, so the favorite-by-name form is not supported here.
    #[arg(long)]
    agent_inline: String,
    /// Seed for deterministic mock responses
    #[arg(long)]
    seed: Option<i64>,
    /// Continuation token from a prior invention response.
    #[arg(long)]
    continuation: Option<String>,
}

/// Fetch a remote invention state via the PluginExecutor.
pub(crate) async fn fetch_state(ref_str: &str) -> Result<ParamsState, crate::error::Error> {
    let executor = crate::objectiveai_executor::executor().await;
    let req = state_get::Request {
        path_type: state_get::Path::FunctionsInventionsStateGet,
        filter: Some(ref_str.to_string()),
        jq: None,
    };
    let resp = state_get::execute(&*executor, req, None)
        .await
        .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("inventions state get: {e}")))?;
    Ok(resp.inner)
}

/// Fill input_schema if it's missing, using our post schema.
pub(crate) fn fill_schema_if_missing(state: ParamsState) -> ParamsState {
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

/// Drive the in-process recursive-invent executor.
///
/// Streams chunks through `OutputResult::Notification` so consumers
/// see incremental progress. Returns `Output::Empty` on success —
/// any terminal id is absorbed into the stream.
pub(crate) async fn run_invention(
    state: &ParamsState,
    fwd: &ForwardArgs,
) -> bool {
    crate::output::emit_result(run_invention_inner(state, fwd).await)
}

async fn run_invention_inner(
    state: &ParamsState,
    fwd: &ForwardArgs,
) -> Result<Output, crate::error::Error> {
    let executor = crate::objectiveai_executor::executor().await;
    let agent: AgentSpec = serde_json::from_str(&fwd.agent_inline)
        .map_err(|e| crate::error::Error::Other(format!("agent_inline parse: {e}")))?;

    let mut advanced = recursive_remote::RequestDangerousAdvanced::default();
    advanced.stream = Some(true);
    let request = RecursiveCreateRequest::Remote(recursive_remote::Request {
        path_type: recursive_remote::Path::FunctionsInventionsRecursiveCreateRemote,
        state: recursive_remote::RequestState::Inline(state.clone()),
        agent,
        continuation: fwd.continuation.clone(),
        seed: fwd.seed,
        dangerous_advanced: Some(advanced),
        jq: None,
    });

    let mut stream = objectiveai_sdk::cli::command::functions::inventions::recursive::create::execute(
        &*executor, request, None,
    )
    .await
    .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("inventions recursive create: {e}")))?;

    while let Some(item) = stream.next().await {
        let item: RecursiveResponseItem = item.map_err(|e| {
            crate::error::Error::ObjectiveAiCli(format!("inventions recursive stream: {e}"))
        })?;
        let value = serde_json::to_value(&item).expect("ResponseItem serializes");
        crate::output::OutputResult::Notification(value).emit();
    }

    Ok(Output::Empty)
}
