use futures::StreamExt;
use objectiveai_sdk::cli::command::functions::{
    executions::create::{
        ResponseItem as CreateResponseItem,
        FunctionSpec, ProfileSpec,
        standard::{self, ResponseItem as StandardItem},
        swiss_system::{self, ResponseItem as SwissItem},
    },
    get as functions_get,
};
use objectiveai_sdk::cli::command::RemotePathCommitOptionalOrFavorite;
use objectiveai_sdk::functions::{
    FullInlineFunctionOrRemoteCommitOptional,
    FullInlineFunction,
    InlineProfileOrRemoteCommitOptional,
};
use objectiveai_sdk::functions::executions::request::Strategy;
use objectiveai_sdk::RemotePathCommitOptional;

use crate::db::Post;
use crate::input::{new_post_input_value, PostsInputValue, PostInputValue};
use crate::psyops::{Stage, is_vector_function};

#[derive(Clone)]
pub struct ScoredPost {
    pub post: Post,
    pub score: f64,
}

/// Fetch a remote function definition and unwrap to inline.
///
/// `functions::get::execute` returns `GetFunctionResponse`, which
/// flattens `RemotePath` and `FullRemoteFunction`. We round-trip the
/// `inner` (the function body) through `serde_json::Value` and parse
/// as `FullInlineFunction` — the wire shape is the same modulo the
/// flattened path metadata, which `FullInlineFunction`'s untagged
/// enum walks past via serde's permissive unknown-field handling.
async fn fetch_function(path: &RemotePathCommitOptional) -> Result<FullInlineFunction, crate::error::Error> {
    let executor = crate::objectiveai_executor::executor().await;
    let req = functions_get::Request {
        path_type: functions_get::Path::FunctionsGet,
        path: RemotePathCommitOptionalOrFavorite::Resolved(path.clone()),
        jq: None,
    };
    let resp = functions_get::execute(&*executor, req, None)
        .await
        .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("functions get: {e}")))?;
    let value = serde_json::to_value(&resp.inner)?;
    let function: FullInlineFunction = serde_json::from_value(value)?;
    Ok(function)
}

async fn resolve_function(function: &FullInlineFunctionOrRemoteCommitOptional) -> Result<FullInlineFunction, crate::error::Error> {
    match function {
        FullInlineFunctionOrRemoteCommitOptional::Inline(f) => Ok(f.clone()),
        FullInlineFunctionOrRemoteCommitOptional::Remote(path) => fetch_function(path).await,
    }
}

/// Run a function execution against the SDK's in-process executor.
/// Dispatches to `standard` or `swiss-system` depending on the
/// psyop's strategy, in streaming mode so we can intercept the
/// terminal chunk's `output`.
///
/// We forward every non-terminal `Chunk` as a notification (matching
/// the old subprocess passthrough), capture the chunk whose `output`
/// field is populated as the terminal value, and emit a Warn-level
/// notification if any of its `tasks_errors` flag is set.
async fn run_function_execution(
    function: &FullInlineFunction,
    profile: &InlineProfileOrRemoteCommitOptional,
    strategy: &Strategy,
    input: objectiveai_sdk::functions::expression::InputValue,
    split: bool,
    invert: bool,
    seed: Option<i64>,
) -> Result<serde_json::Value, crate::error::Error> {
    let executor = crate::objectiveai_executor::executor().await;
    let function_spec = FunctionSpec::Resolved(
        FullInlineFunctionOrRemoteCommitOptional::Inline(function.clone()),
    );
    let profile_spec = ProfileSpec::Resolved(profile.clone());

    let request = match strategy {
        Strategy::Default => {
            let mut advanced = standard::RequestDangerousAdvanced::default();
            advanced.stream = Some(true);
            objectiveai_sdk::cli::command::functions::executions::create::Request::Standard(
                standard::Request {
                    path_type: standard::Path::FunctionsExecutionsCreateStandard,
                    function: function_spec,
                    profile: profile_spec,
                    input: standard::RequestInput::Inline(input),
                    continuation: None,
                    retry_token: None,
                    seed,
                    split,
                    invert,
                    dangerous_advanced: Some(advanced),
                    jq: None,
                },
            )
        }
        Strategy::SwissSystem { pool, rounds } => {
            let mut advanced = swiss_system::RequestDangerousAdvanced::default();
            advanced.stream = Some(true);
            objectiveai_sdk::cli::command::functions::executions::create::Request::SwissSystem(
                swiss_system::Request {
                    path_type: swiss_system::Path::FunctionsExecutionsCreateSwissSystem,
                    function: function_spec,
                    profile: profile_spec,
                    input: swiss_system::RequestInput::Inline(input),
                    continuation: None,
                    retry_token: None,
                    seed,
                    split,
                    invert,
                    pool: *pool,
                    rounds: *rounds,
                    dangerous_advanced: Some(advanced),
                    jq: None,
                },
            )
        }
    };

    let mut stream = objectiveai_sdk::cli::command::functions::executions::create::execute(
        &*executor, request, None,
    )
    .await
    .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("executions create: {e}")))?;

    let mut terminal: Option<serde_json::Value> = None;

    while let Some(item) = stream.next().await {
        let item = item
            .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("executions create stream: {e}")))?;
        let (output, tasks_errors, value) = match &item {
            CreateResponseItem::Standard(StandardItem::Chunk(c)) => (
                c.output.as_ref().map(|o| serde_json::to_value(&o.output).expect("output serializes")),
                c.tasks_errors.unwrap_or(false),
                serde_json::to_value(&item).expect("ResponseItem serializes"),
            ),
            CreateResponseItem::SwissSystem(SwissItem::Chunk(c)) => (
                c.output.as_ref().map(|o| serde_json::to_value(&o.output).expect("output serializes")),
                c.tasks_errors.unwrap_or(false),
                serde_json::to_value(&item).expect("ResponseItem serializes"),
            ),
            _ => (None, false, serde_json::to_value(&item).expect("ResponseItem serializes")),
        };

        if tasks_errors {
            crate::output::OutputResult::from(crate::events::Event::ObjectiveaiTaskErrors {
                count: 1,
            })
            .emit();
        }

        match output {
            Some(o) if terminal.is_none() => {
                terminal = Some(o);
            }
            _ => {
                // Forward the chunk verbatim as a notification so
                // consumers see progress.
                crate::output::OutputResult::Notification(value).emit();
            }
        }
    }

    terminal.ok_or_else(|| crate::error::Error::ObjectiveAiCli(
        "executions create produced no terminal chunk with output".into(),
    ))
}

/// Run a single stage's function execution against the given posts.
/// Returns scored posts in score-descending order.
pub async fn score(stage: &Stage, posts: Vec<Post>, seed: Option<i64>) -> Result<Vec<ScoredPost>, crate::error::Error> {
    let mut scored: Vec<ScoredPost> = posts.into_iter()
        .map(|p| ScoredPost { post: p, score: 0.0 })
        .collect();

    let function = resolve_function(&stage.function).await?;
    let is_vector = is_vector_function(&function);

    let items: Vec<PostInputValue> = scored.iter()
        .map(|s| new_post_input_value(&s.post, stage.images, stage.videos))
        .collect();

    let (input_value, split) = if is_vector {
        let input = PostsInputValue { items };
        (serde_json::to_value(&input)?, false)
    } else {
        (serde_json::to_value(&items)?, true)
    };

    let input_value: objectiveai_sdk::functions::expression::InputValue =
        serde_json::from_value(input_value)?;

    let result = run_function_execution(
        &function, &stage.profile, &stage.strategy, input_value,
        split, stage.invert, seed,
    ).await?;

    let scores: Vec<f64> = result.as_array()
        .ok_or_else(|| crate::error::Error::Other(
            format!("expected array score output, got {result}"),
        ))?
        .iter()
        .map(|v| v.as_f64().ok_or_else(|| crate::error::Error::Other(
            format!("expected numeric score, got {v}"),
        )))
        .collect::<Result<Vec<_>, _>>()?;

    if scores.len() != scored.len() {
        return Err(crate::error::Error::Other(
            format!("score count ({}) doesn't match post count ({})", scores.len(), scored.len()),
        ));
    }

    for (s, val) in scored.iter_mut().zip(scores.iter()) {
        s.score = *val;
    }

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    Ok(scored)
}
