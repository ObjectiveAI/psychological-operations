use objectiveai_sdk::functions::{
    FullInlineFunctionOrRemoteCommitOptional,
    FullInlineFunction,
    InlineProfileOrRemoteCommitOptional,
};
use objectiveai_sdk::functions::executions::request::Strategy;
use objectiveai_sdk::RemotePathCommitOptional;
use serde::Deserialize;

use crate::db::Post;
use crate::input::{new_post_input_value, PostsInputValue, PostInputValue};
use crate::psyops::{Stage, is_vector_function};

#[derive(Clone)]
pub struct ScoredPost {
    pub post: Post,
    pub score: f64,
}

#[derive(Deserialize)]
struct ExecutionOutput {
    output: serde_json::Value,
}

/// Format a RemotePathCommitOptional for the CLI --path argument.
/// The CLI expects `key=value,key=value` format, not JSON.
fn format_remote_ref(path: &RemotePathCommitOptional) -> String {
    match path {
        RemotePathCommitOptional::Github { owner, repository, commit } => {
            let mut s = format!("remote=github,owner={owner},repository={repository}");
            if let Some(c) = commit {
                s.push_str(&format!(",commit={c}"));
            }
            s
        }
        RemotePathCommitOptional::Filesystem { owner, repository, commit } => {
            let mut s = format!("remote=filesystem,owner={owner},repository={repository}");
            if let Some(c) = commit {
                s.push_str(&format!(",commit={c}"));
            }
            s
        }
        RemotePathCommitOptional::Mock { name } => {
            format!("remote=mock,name={name}")
        }
    }
}

/// Locate the objectiveai CLI used for subprocess scoring calls
/// (`functions get`, `executions create`, …).
///
/// Resolution order:
///   1. `PSYCHOLOGICAL_OPERATIONS_OBJECTIVEAI_BINARY` env var
///      — explicit override. The integration-test harness uses this
///      to point at a no-viewer release binary so test runs don't
///      spawn viewer windows.
///   2. `<HOME|USERPROFILE>/.objectiveai/objectiveai[.exe]` — the
///      install script's default location. The Windows installer
///      only updates user-env PATH, which isn't reflected in
///      already-running shells, so this fallback is what makes the
///      bare `objectiveai` invocation work after a fresh install.
///   3. `PATH` lookup (last-resort bare name).
pub fn objectiveai_binary(_cfg: &crate::run::Config) -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Ok(p) = std::env::var("PSYCHOLOGICAL_OPERATIONS_OBJECTIVEAI_BINARY") {
        return PathBuf::from(p);
    }
    let name = if cfg!(windows) { "objectiveai.exe" } else { "objectiveai" };
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        let candidate = PathBuf::from(home).join(".objectiveai").join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(name)
}

/// Spawn an objectiveai CLI subprocess, stream its stdout line-by-line
/// to **our** stdout (so the host upstream of us sees every progress
/// notification the inner CLI emits), and return the `value` payload of
/// the first `notification` whose `value` carries the requested
/// `terminal_key` top-level field.
///
/// Forwarding rules:
/// - `Output::Begin` / `Output::End` — DROPPED. The host already
///   bookends our own stream; inner subprocess bookends would
///   duplicate.
/// - `Output::Notification(value)` where `value.<terminal_key>` is
///   present — captured as the return value. Not forwarded (the caller
///   uses it directly).
/// - `Output::Notification(value)` otherwise — forwarded verbatim via
///   `emit_notification` (e.g. `log_stream_ready`, streamed
///   `inner_errors`).
/// - `Output::Error(err)` — forwarded via `emit_error`. Fatal errors
///   propagate as `Err(_)`.
/// - Any line that fails to parse — forwarded as a raw-string
///   notification so non-JSONL warnings still surface.
fn run_objectiveai_subprocess<I, S>(
    args: I,
    terminal_key: &str,
    cfg: &crate::run::Config,
) -> Result<serde_json::Value, crate::error::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    use std::io::{BufRead, BufReader};
    use objectiveai_cli_sdk::output::{Notification as CliNotif, Output as CliOutput};

    let mut child = std::process::Command::new(objectiveai_binary(cfg))
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;

    let stdout = child.stdout.take().expect("piped stdout");
    let reader = BufReader::new(stdout);

    let mut terminal: Option<serde_json::Value> = None;

    for line in reader.lines() {
        let line = line.map_err(|e| crate::error::Error::ObjectiveAiCli(
            format!("read subprocess stdout: {e}"),
        ))?;
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        match serde_json::from_str::<CliOutput<serde_json::Value>>(trimmed) {
            Ok(CliOutput::Begin) | Ok(CliOutput::End) => {
                // Inner bookends — drop. The host already wraps us.
            }
            Ok(CliOutput::Notification(CliNotif { value })) => {
                let is_terminal = value.is_object()
                    && value.get(terminal_key).is_some();
                if is_terminal {
                    terminal = Some(value);
                } else if value.is_object() {
                    crate::emit::emit_notification(value);
                } else {
                    // Non-object value (rare). Wrap so PluginOutput's
                    // internal tagging can carry it.
                    crate::emit::emit_notification(serde_json::json!({"value": value}));
                }
            }
            Ok(CliOutput::Error(err)) => {
                let fatal = err.fatal;
                let message_for_err = err.message.clone();
                crate::emit::emit_error(err.level, err.fatal, err.message);
                if fatal {
                    let _ = child.wait();
                    let msg = message_for_err.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| message_for_err.to_string());
                    return Err(crate::error::Error::ObjectiveAiCli(msg));
                }
            }
            Err(_) => {
                // Non-JSONL line (e.g. a stray warning). Forward
                // visibly under a `raw` key so it's still in the
                // stream.
                crate::emit::emit_notification(serde_json::json!({"raw": trimmed}));
            }
        }
    }

    let status = child.wait()?;
    if !status.success() && terminal.is_none() {
        return Err(crate::error::Error::ObjectiveAiCli(
            format!("subprocess exited non-zero with no `{terminal_key}` notification"),
        ));
    }

    terminal.ok_or_else(|| crate::error::Error::ObjectiveAiCli(
        format!("subprocess produced no `notification` carrying `{terminal_key}`"),
    ))
}

/// Fetch a remote function definition via the CLI and deserialize to inline.
///
/// Terminal notification shape:
///   `{"function": <GetFunctionResponse>}`
/// `GetFunctionResponse` flattens `RemotePath` (remote/owner/…)
/// alongside the function body (`type`, `description`, `input_schema`,
/// `tasks`). We pluck the `function` field, drop the path metadata by
/// deserializing as `FullInlineFunction` (serde ignores unknown fields),
/// and rely on `InlineFunction` accepting the extra `description` /
/// `input_schema` keys without complaint.
fn fetch_function(path: &RemotePathCommitOptional, cfg: &crate::run::Config) -> Result<FullInlineFunction, crate::error::Error> {
    let ref_str = format_remote_ref(path);
    let value = run_objectiveai_subprocess(
        ["functions", "get", "--path", &ref_str],
        "function",
        cfg,
    )?;
    let function_value = &value["function"];
    let function: FullInlineFunction = serde_json::from_value(function_value.clone())?;
    Ok(function)
}

/// Resolve a function to its inline definition, fetching if remote.
fn resolve_function(function: &FullInlineFunctionOrRemoteCommitOptional, cfg: &crate::run::Config) -> Result<FullInlineFunction, crate::error::Error> {
    match function {
        FullInlineFunctionOrRemoteCommitOptional::Inline(f) => Ok(f.clone()),
        FullInlineFunctionOrRemoteCommitOptional::Remote(path) => fetch_function(path, cfg),
    }
}

/// Fetch a fresh function-execution `--instructions-id` from the CLI. The
/// objectiveai CLI requires this token on every `executions create` call;
/// it ties a specific execution to the current instructions revision.
///
/// Wire: `{"type":"notification","value":{"instructions":"<markdown>"}}`.
/// The markdown body ends with an `Instructions ID: <id>` line — extract
/// that ID and return it.
fn fetch_instructions_id(cfg: &crate::run::Config) -> Result<String, crate::error::Error> {
    let value = run_objectiveai_subprocess(
        ["functions", "executions", "instructions", "get"],
        "instructions",
        cfg,
    )?;
    let instructions = value["instructions"].as_str().ok_or_else(|| {
        crate::error::Error::ObjectiveAiCli(
            format!("instructions get notification `instructions` field is not a string: {value}"),
        )
    })?;
    let id = instructions.lines().rev()
        .find_map(|l| l.trim().strip_prefix("Instructions ID:").map(|s| s.trim().to_string()))
        .ok_or_else(|| crate::error::Error::ObjectiveAiCli(
            format!("instructions body missing `Instructions ID:` line: {instructions}"),
        ))?;
    Ok(id)
}

/// Run a function execution via the CLI. Dispatches to either the
/// `standard` or `swiss-system` subcommand based on the psyop's strategy,
/// fetches a fresh `--instructions-id` for the call, and always passes the
/// inline function + profile.
fn run_function_execution(
    function: &FullInlineFunction,
    profile: &InlineProfileOrRemoteCommitOptional,
    strategy: &Strategy,
    input_json: &str,
    split: bool,
    invert: bool,
    seed: Option<i64>,
    cfg: &crate::run::Config,
) -> Result<ExecutionOutput, crate::error::Error> {
    let function_json = serde_json::to_string(function)?;
    let profile_json = serde_json::to_string(profile)?;
    let instructions_id = fetch_instructions_id(cfg)?;

    let subcommand = match strategy {
        Strategy::Default => "standard",
        Strategy::SwissSystem { .. } => "swiss-system",
    };

    let mut args = vec![
        "functions".to_string(), "executions".to_string(), "create".to_string(), subcommand.to_string(),
        "--instructions-id".to_string(), instructions_id,
        "--function-inline".to_string(), function_json,
        "--profile-inline".to_string(), profile_json,
        "--input-inline".to_string(), input_json.to_string(),
    ];

    if let Strategy::SwissSystem { pool, rounds } = strategy {
        if let Some(p) = pool {
            args.push("--pool".to_string());
            args.push(p.to_string());
        }
        if let Some(r) = rounds {
            args.push("--rounds".to_string());
            args.push(r.to_string());
        }
    }

    if split {
        args.push("--split".to_string());
    }
    if invert {
        args.push("--invert".to_string());
    }
    if let Some(s) = seed {
        args.push("--seed".to_string());
        args.push(s.to_string());
    }

    let value = run_objectiveai_subprocess(args, "execution", cfg)?;

    // Terminal notification wire (v2.0.5):
    //   {"execution":{"output":<TaskOutputOwned>,"errors":[...]}}
    // `TaskOutputOwned` (defined in
    // `objectiveai-sdk-rs/src/functions/expression/params.rs`) is an
    // untagged enum: scalar → number, vector → array, vectors →
    // nested arrays, error → object. For our score path we expect an
    // array (vector or split-scalar).
    let execution = &value["execution"];
    let errors = execution.get("errors").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    if !errors.is_empty() {
        // Silent fallback to a uniform prior. Keep the message
        // byte-stable across runs by omitting any per-execution IDs.
        eprintln!(
            "warning: objectiveai execution reported {} task error(s) \
             — output may be a fallback",
            errors.len(),
        );
    }
    let inner = execution.get("output").cloned().ok_or_else(|| {
        crate::error::Error::ObjectiveAiCli(
            format!("executions create notification missing `execution.output`: {value}"),
        )
    })?;

    Ok(ExecutionOutput { output: inner })
}

/// Run a single stage's function execution against the given posts.
/// Returns scored posts in score-descending order.
pub fn score(stage: &Stage, posts: Vec<Post>, seed: Option<i64>, cfg: &crate::run::Config) -> Result<Vec<ScoredPost>, crate::error::Error> {
    let mut scored: Vec<ScoredPost> = posts.into_iter()
        .map(|p| ScoredPost { post: p, score: 0.0 })
        .collect();

    eprintln!("Scoring {} posts...", scored.len());

    let function = resolve_function(&stage.function, cfg)?;
    let is_vector = is_vector_function(&function);

    let items: Vec<PostInputValue> = scored.iter()
        .map(|s| new_post_input_value(&s.post, stage.images, stage.videos))
        .collect();

    let (input_json, split) = if is_vector {
        let input = PostsInputValue { items };
        (serde_json::to_string(&input)?, false)
    } else {
        (serde_json::to_string(&items)?, true)
    };

    let result = run_function_execution(&function, &stage.profile, &stage.strategy, &input_json, split, stage.invert, seed, cfg)?;

    let scores: Vec<f64> = result.output.as_array()
        .ok_or_else(|| crate::error::Error::Other(
            format!("expected array score output, got {}", result.output),
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
