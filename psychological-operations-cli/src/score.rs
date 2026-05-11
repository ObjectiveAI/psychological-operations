use objectiveai::functions::{
    FullInlineFunctionOrRemoteCommitOptional,
    FullInlineFunction,
    InlineProfileOrRemoteCommitOptional,
};
use objectiveai::functions::executions::request::Strategy;
use objectiveai::RemotePathCommitOptional;
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

/// Locate the objectiveai CLI. Prefer PATH; fall back to the install script's
/// default location at ~/.objectiveai/objectiveai(.exe) — the Windows installer
/// only updates the user environment PATH, which isn't reflected in an already-
/// running shell.
pub fn objectiveai_binary(_cfg: &crate::run::Config) -> std::path::PathBuf {
    use std::path::PathBuf;
    let name = if cfg!(windows) { "objectiveai.exe" } else { "objectiveai" };
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        let candidate = PathBuf::from(home).join(".objectiveai").join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(name)
}

/// Fetch a remote function definition via the CLI and deserialize to inline.
fn fetch_function(path: &RemotePathCommitOptional, cfg: &crate::run::Config) -> Result<FullInlineFunction, crate::error::Error> {
    let ref_str = format_remote_ref(path);
    let output = std::process::Command::new(objectiveai_binary(cfg))
        .args(["functions", "get", "--path", &ref_str])
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()?;

    if !output.status.success() {
        return Err(crate::error::Error::ObjectiveAiCli("failed to fetch function".into()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let function: FullInlineFunction = serde_json::from_str(stdout.trim())?;
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
fn fetch_instructions_id(cfg: &crate::run::Config) -> Result<String, crate::error::Error> {
    let output = std::process::Command::new(objectiveai_binary(cfg))
        .args(["functions", "executions", "instructions", "get"])
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()?;
    if !output.status.success() {
        return Err(crate::error::Error::ObjectiveAiCli(
            format!("instructions get failed: {}", String::from_utf8_lossy(&output.stdout)),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The CLI prints a long preamble plus a final " Instructions ID: <id>" line.
    let id = stdout.lines().rev()
        .find_map(|l| l.trim().strip_prefix("Instructions ID:").map(|s| s.trim().to_string()))
        .ok_or_else(|| crate::error::Error::ObjectiveAiCli(
            format!("instructions get returned no ID line: {stdout}"),
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

    let output = std::process::Command::new(objectiveai_binary(cfg))
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()?;

    if !output.status.success() {
        return Err(crate::error::Error::ObjectiveAiCli(
            String::from_utf8_lossy(&output.stdout).to_string(),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Print stdout passthrough (log lines etc.)
    print!("{stdout}");

    // The objectiveai CLI prints `Logs ID: <id>` to stdout the first
    // time its writer task flushes a chunk to disk. The final score
    // lives in the per-execution log file at
    // <logs_dir>/functions/executions/<id>.json under
    // ["output"]["output"]. Reading the log file is more robust than
    // scanning stdout for the final result JSON, since stdout may
    // contain other lines (viewer info, warnings, future telemetry).
    let id = stdout.lines()
        .find_map(|l| l.trim().strip_prefix("Logs ID:").map(|s| s.trim().to_string()))
        .ok_or_else(|| crate::error::Error::ObjectiveAiCli(
            format!("no `Logs ID:` line in stdout: {stdout}"),
        ))?;

    let log_path = objectiveai_logs_dir()
        .join("functions").join("executions")
        .join(format!("{id}.json"));

    let bytes = std::fs::read(&log_path).map_err(|e| {
        crate::error::Error::ObjectiveAiCli(format!(
            "failed to read execution log {}: {e}", log_path.display(),
        ))
    })?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        crate::error::Error::ObjectiveAiCli(format!(
            "failed to parse execution log {}: {e}", log_path.display(),
        ))
    })?;

    // Surface execution-level failures: a successful subprocess +
    // present log file + parseable `output.output` is not the same
    // as "scoring worked". When tasks fail, ObjectiveAI falls back
    // to a uniform prior — silently, exit 0. Warn early so the
    // fallback values aren't mistaken for real signal. The
    // `Logs ID: <id>` line already on stdout provides traceability;
    // we omit the id here to keep the warning byte-stable across runs.
    if root.get("tasks_errors").and_then(|v| v.as_bool()) == Some(true) {
        eprintln!(
            "warning: objectiveai execution flagged tasks_errors \
             (one or more tasks errored — output is likely a fallback)"
        );
    }
    if let Some(err) = root.get("error").filter(|v| !v.is_null()) {
        let body = serde_json::to_string(err)
            .unwrap_or_else(|_| err.to_string());
        eprintln!("warning: objectiveai execution error: {body}");
    }

    let inner = root.get("output").and_then(|o| o.get("output")).cloned()
        .ok_or_else(|| crate::error::Error::ObjectiveAiCli(format!(
            "execution log {} missing [\"output\"][\"output\"]", log_path.display(),
        )))?;

    Ok(ExecutionOutput { output: inner })
}

/// Mirrors `objectiveai::filesystem::Client::new` base-dir resolution
/// (with the `env` feature): explicit > `CONFIG_BASE_DIR` env >
/// `~/.objectiveai`. The objectiveai dep is pulled in with
/// default-features = false, so we re-derive here rather than flip
/// feature flags.
fn objectiveai_logs_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("CONFIG_BASE_DIR") {
        return std::path::PathBuf::from(d).join("logs");
    }
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".objectiveai")
        .join("logs")
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
        .ok_or_else(|| crate::error::Error::Other("expected array output".into()))?
        .iter()
        .map(|v| v.as_f64().unwrap_or(0.0))
        .collect();

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
