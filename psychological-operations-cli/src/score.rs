use futures::StreamExt;
use objectiveai_sdk::cli::command::functions::{
    execute::{
        self,
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
use crate::psyops::is_vector_function;

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
async fn fetch_function(
    path: &RemotePathCommitOptional,
    ctx: &crate::context::Context,
) -> Result<FullInlineFunction, crate::error::Error> {
    let executor = ctx.executor.clone();
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

async fn resolve_function(
    function: &FullInlineFunctionOrRemoteCommitOptional,
    ctx: &crate::context::Context,
) -> Result<FullInlineFunction, crate::error::Error> {
    match function {
        FullInlineFunctionOrRemoteCommitOptional::Inline(f) => Ok(f.clone()),
        FullInlineFunctionOrRemoteCommitOptional::Remote(path) => fetch_function(path, ctx).await,
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
    ctx: &crate::context::Context,
) -> Result<serde_json::Value, crate::error::Error> {
    let executor = ctx.executor.clone();

    // Pass the function / profile / input as FILE PATHS, not inline
    // JSON. The SDK's in-process `PluginExecutor` emits a nested
    // command as `argv.join(" ")` and the objectiveai host re-splits
    // it with `split_whitespace` (no shlex) — so any argument that
    // contains a space is shattered into separate tokens and clap
    // rejects the reconstructed command. The scoring input carries
    // tweet text (and the function / profile carry instruction
    // prose), all of which contain spaces. Writing each to a temp
    // file and passing its (space-free) path is the only encoding
    // that survives the protocol. Files live until the nested
    // command has been served; the `_tmp` guard reaps them on return.
    let tmp = ExecTempDir::new(ctx)?;
    let fn_path = tmp.write_json(
        "function.json",
        &FullInlineFunctionOrRemoteCommitOptional::Inline(function.clone()),
    )?;
    let profile_path = tmp.write_json("profile.json", profile)?;
    let input_path = tmp.write_json("input.json", &input)?;

    let function_spec = FunctionSpec::File(fn_path);
    let profile_spec = ProfileSpec::File(profile_path);

    let request = match strategy {
        Strategy::Default => {
            let advanced = standard::RequestDangerousAdvanced {
                stream: Some(true),
                seed,
            };
            execute::Request::Standard(standard::Request {
                path_type: standard::Path::FunctionsExecuteStandard,
                function: function_spec,
                profile: profile_spec,
                input: standard::RequestInput::File(input_path),
                continuation: None,
                retry_token: None,
                split,
                invert,
                dangerous_advanced: Some(advanced),
                jq: None,
            })
        }
        Strategy::SwissSystem { pool, rounds } => {
            let advanced = swiss_system::RequestDangerousAdvanced {
                stream: Some(true),
                seed,
            };
            execute::Request::SwissSystem(swiss_system::Request {
                path_type: swiss_system::Path::FunctionsExecuteSwissSystem,
                function: function_spec,
                profile: profile_spec,
                input: swiss_system::RequestInput::File(input_path),
                continuation: None,
                retry_token: None,
                split,
                invert,
                pool: *pool,
                rounds: *rounds,
                dangerous_advanced: Some(advanced),
                jq: None,
            })
        }
    };

    let mut stream = execute::execute(&*executor, request, None)
        .await
        .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("functions execute: {e}")))?;

    let mut terminal: Option<serde_json::Value> = None;

    while let Some(item) = stream.next().await {
        let item = item
            .map_err(|e| crate::error::Error::ObjectiveAiCli(format!("functions execute stream: {e}")))?;
        let (output, tasks_errors) = match &item {
            CreateResponseItem::Standard(StandardItem::Chunk(c)) => (
                c.output.as_ref().map(|o| serde_json::to_value(&o.output).expect("output serializes")),
                c.tasks_errors.unwrap_or(false),
            ),
            CreateResponseItem::SwissSystem(SwissItem::Chunk(c)) => (
                c.output.as_ref().map(|o| serde_json::to_value(&o.output).expect("output serializes")),
                c.tasks_errors.unwrap_or(false),
            ),
            _ => (None, false),
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
                // Stop at the terminal chunk — never drain an
                // executor stream to its end. The host writes a
                // nested command's stream-terminating marker only
                // after the plugin's stdout EOFs, so waiting for
                // "no more items" here deadlocks (we wait on the
                // host, the host waits on our exit).
                break;
            }
            // Non-terminal chunks are NOT forwarded. They're a
            // per-vote streaming firehose carrying wall-clock
            // timestamps and random ids (hundreds per execution) —
            // non-deterministic noise, not progress a consumer can
            // act on. The deterministic `stage_begin` / `stage_end`
            // events (emitted by `score_pipeline`) mark progress
            // instead. Only the terminal output is retained.
            _ => {}
        }
    }

    terminal.ok_or_else(|| crate::error::Error::ObjectiveAiCli(
        "functions execute produced no terminal chunk with output".into(),
    ))
}

/// Throwaway directory holding one function-execution's file-passed
/// args (function / profile / input JSON). Lives under
/// `<base>/plugins-state/psychological-operations/exec-tmp/<pid>-<seq>/`
/// — a path with no spaces, so the nested `functions execute` command
/// the host reconstructs by `split_whitespace` can carry it intact.
/// Removed on drop, which the caller holds until after the execution
/// stream is fully consumed (the host reads the files while serving
/// the nested command).
struct ExecTempDir {
    dir: std::path::PathBuf,
}

impl ExecTempDir {
    fn new(ctx: &crate::context::Context) -> Result<Self, crate::error::Error> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = ctx
            .config
            .objectiveai_base_dir()
            .join("plugins-state")
            .join("psychological-operations")
            .join("exec-tmp")
            .join(format!("{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn write_json<T: serde::Serialize>(
        &self,
        name: &str,
        value: &T,
    ) -> Result<std::path::PathBuf, crate::error::Error> {
        let path = self.dir.join(name);
        std::fs::write(&path, serde_json::to_string(value)?)?;
        Ok(path)
    }
}

impl Drop for ExecTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run a single function-stage's objectiveai execution against
/// the given posts. Returns scored posts in score-descending
/// order. The `Stage::Bare` variant skips this entirely — the
/// score_pipeline caller assigns flat 1.0 instead.
#[allow(clippy::too_many_arguments)]
pub async fn score_function(
    function_spec: &FullInlineFunctionOrRemoteCommitOptional,
    profile:       &InlineProfileOrRemoteCommitOptional,
    strategy:      &Strategy,
    invert:        bool,
    images:        bool,
    videos:        bool,
    posts:         Vec<Post>,
    seed:          Option<i64>,
    ctx:           &crate::context::Context,
) -> Result<Vec<ScoredPost>, crate::error::Error> {
    let mut scored: Vec<ScoredPost> = posts.into_iter()
        .map(|p| ScoredPost { post: p, score: 0.0 })
        .collect();

    let function = resolve_function(function_spec, ctx).await?;
    let is_vector = is_vector_function(&function);

    let items: Vec<PostInputValue> = scored.iter()
        .map(|s| new_post_input_value(&s.post, images, videos))
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
        &function, profile, strategy, input_value,
        split, invert, seed, ctx,
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
