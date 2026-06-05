//! Recursive function-invention dispatch for the `functions
//! invent` subcommand. Surface lives in
//! `crate::commands::functions::invent` — operator-visible args
//! come straight from the objectiveai SDK's clap structs. This
//! module owns the runtime: it converts the per-leaf `Args` into a
//! [`recursive_remote::Request`] (because that's the only SDK
//! variant whose `RequestState` carries the [`ParamsState`]
//! envelope where psyops slots its post input-schema), then
//! dispatches via the in-process [`PluginExecutor`].
//!
//! Streaming-vs-unary routing is taken verbatim from the operator's
//! `--dangerous-advanced '{"stream":true|false}'` flag. We never
//! force a value — `recursive_remote::execute_streaming` sets
//! `stream = Some(true)` itself, and `recursive_remote::execute`
//! clears it — so we just check the parsed value to pick which
//! call site to enter.
//!
//! Wire shape:
//! - Streaming: each [`ResponseItem`] lands as
//!   [`OutputResult::Notification`]; terminal output is
//!   [`Output::Empty`] (the id is the last chunk's job).
//! - Unary: terminal output is [`Output::Api(id)`] which
//!   `emit_output` re-wraps as a Notification carrying the id.

use futures::StreamExt;
use objectiveai_sdk::cli::command::agents::spawn::AgentSpec;
use objectiveai_sdk::cli::command::functions::inventions::{
    recursive::create::{
        alpha_scalar, alpha_vector,
        remote as recursive_remote,
    },
    state::get as state_get,
};
use objectiveai_sdk::functions::inventions::{
    ParamsState,
    state::{AlphaScalarState, AlphaVectorState, Params},
};
use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::input;

// ─── Per-leaf entry points (called by commands::functions::invent) ──

/// `functions invent alpha-scalar`. Converts the SDK's parsed
/// args into a `ParamsState::AlphaScalar` envelope carrying the
/// scalar post-input-schema, then dispatches via the shared
/// `recursive_remote` path.
pub async fn invent_alpha_scalar(args: alpha_scalar::Args) -> bool {
    crate::output::emit_result(invent_alpha_scalar_inner(args).await)
}

async fn invent_alpha_scalar_inner(args: alpha_scalar::Args) -> Result<Output, Error> {
    let req = alpha_scalar::Request::try_from(args)
        .map_err(|e| Error::Other(format!("alpha-scalar args: {e}")))?;
    let state = ParamsState::AlphaScalar(AlphaScalarState {
        params: Params {
            depth:            req.params.depth,
            min_branch_width: req.params.min_branch_width,
            max_branch_width: req.params.max_branch_width,
            min_leaf_width:   req.params.min_leaf_width,
            max_leaf_width:   req.params.max_leaf_width,
            name:             req.params.name,
            spec:             req.params.spec,
        },
        input_schema: Some(input::scalar_input_schema()),
    });
    let dangerous_advanced = req.dangerous_advanced.map(|a| {
        recursive_remote::RequestDangerousAdvanced { stream: a.stream }
    });
    dispatch_remote(
        state,
        req.agent,
        req.continuation,
        req.seed,
        dangerous_advanced,
        req.jq,
    ).await
}

/// `functions invent alpha-vector`. Mirror of
/// `invent_alpha_scalar` for the vector schema variant.
pub async fn invent_alpha_vector(args: alpha_vector::Args) -> bool {
    crate::output::emit_result(invent_alpha_vector_inner(args).await)
}

async fn invent_alpha_vector_inner(args: alpha_vector::Args) -> Result<Output, Error> {
    let req = alpha_vector::Request::try_from(args)
        .map_err(|e| Error::Other(format!("alpha-vector args: {e}")))?;
    let state = ParamsState::AlphaVector(AlphaVectorState {
        params: Params {
            depth:            req.params.depth,
            min_branch_width: req.params.min_branch_width,
            max_branch_width: req.params.max_branch_width,
            min_leaf_width:   req.params.min_leaf_width,
            max_leaf_width:   req.params.max_leaf_width,
            name:             req.params.name,
            spec:             req.params.spec,
        },
        input_schema: Some(input::vector_input_schema()),
    });
    let dangerous_advanced = req.dangerous_advanced.map(|a| {
        recursive_remote::RequestDangerousAdvanced { stream: a.stream }
    });
    dispatch_remote(
        state,
        req.agent,
        req.continuation,
        req.seed,
        dangerous_advanced,
        req.jq,
    ).await
}

/// `functions invent remote`. Resolves the operator's `--state`
/// ref via [`state_get`] (or accepts the `--state-inline` JSON
/// directly), injects the psyops schema if the resolved state
/// doesn't carry one, then dispatches via `recursive_remote`.
pub async fn invent_remote(args: recursive_remote::Args) -> bool {
    crate::output::emit_result(invent_remote_inner(args).await)
}

async fn invent_remote_inner(args: recursive_remote::Args) -> Result<Output, Error> {
    let req = recursive_remote::Request::try_from(args)
        .map_err(|e| Error::Other(format!("remote args: {e}")))?;
    let state = match req.state {
        recursive_remote::RequestState::Inline(state) => fill_schema_if_missing(state),
        recursive_remote::RequestState::Ref(ref_str)  => {
            let fetched = fetch_state(&ref_str).await?;
            fill_schema_if_missing(fetched)
        }
    };
    dispatch_remote(
        state,
        req.agent,
        req.continuation,
        req.seed,
        req.dangerous_advanced,
        req.jq,
    ).await
}

// ─── Shared dispatch + helpers ──────────────────────────────────

/// Build a `recursive_remote::Request` from the resolved state +
/// pass-through fields, then route to streaming or unary SDK
/// dispatch depending on the operator-supplied
/// `dangerous_advanced.stream` flag.
async fn dispatch_remote(
    state: ParamsState,
    agent: AgentSpec,
    continuation: Option<String>,
    seed: Option<i64>,
    dangerous_advanced: Option<recursive_remote::RequestDangerousAdvanced>,
    jq: Option<String>,
) -> Result<Output, Error> {
    let executor = crate::objectiveai_executor::executor().await;
    let stream_requested = dangerous_advanced
        .as_ref()
        .and_then(|a| a.stream)
        .unwrap_or(false);
    let request = recursive_remote::Request {
        path_type: recursive_remote::Path::FunctionsInventionsRecursiveCreateRemote,
        state: recursive_remote::RequestState::Inline(state),
        agent,
        continuation,
        seed,
        dangerous_advanced,
        jq,
    };

    if stream_requested {
        let mut stream = recursive_remote::execute_streaming(&*executor, request, None)
            .await
            .map_err(|e| Error::ObjectiveAiCli(format!("inventions recursive create: {e}")))?;
        while let Some(item) = stream.next().await {
            let item = item.map_err(|e| {
                Error::ObjectiveAiCli(format!("inventions recursive stream: {e}"))
            })?;
            crate::output::OutputResult::Notification(
                serde_json::to_value(&item).expect("ResponseItem serializes"),
            )
            .emit();
        }
        Ok(Output::Empty)
    } else {
        let id = recursive_remote::execute(&*executor, request, None)
            .await
            .map_err(|e| Error::ObjectiveAiCli(format!("inventions recursive create: {e}")))?;
        Ok(Output::Api(id))
    }
}

/// Fetch a remote invention state via the in-process executor —
/// used by the `remote` leaf when the operator supplies `--state
/// <ref>` instead of `--state-inline` (so psyops can inject its
/// schema before redispatching).
async fn fetch_state(ref_str: &str) -> Result<ParamsState, Error> {
    let executor = crate::objectiveai_executor::executor().await;
    let req = state_get::Request {
        path_type: state_get::Path::FunctionsInventionsStateGet,
        filter: Some(ref_str.to_string()),
        jq: None,
    };
    let resp = state_get::execute(&*executor, req, None)
        .await
        .map_err(|e| Error::ObjectiveAiCli(format!("inventions state get: {e}")))?;
    Ok(resp.inner)
}

/// Inject psyops's post input-schema (scalar or vector) into any
/// [`ParamsState`] variant whose `input_schema` is currently
/// `None`. Variants that already carry one are passed through
/// untouched.
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
