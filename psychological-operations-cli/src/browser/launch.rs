//! Spawn the extracted psychological-operations-browser binary in
//! a given mode. Caller has already resolved the mode (and any
//! psyop/agent name) and called [`super::extract::ensure_extracted`].

use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::error::Error;

/// CLI-side mirror of the browser's `--<mode>` flag set. We don't
/// reach for the SDK's [`psychological_operations_sdk::browser::mode::Mode`]
/// here because the SDK enum carries no inherent CLI representation
/// (it's used renderer-side for `__PSYOPS_MODE` injection); the
/// browser's clap layer in `args.rs` is the source of truth for the
/// flag spellings and we mirror it.
pub enum Mode {
    XApp,
    PsyopRead { name: String },
    PsyopAuthorize { name: String },
    AgentAuthorize { name: String },
    PsyopBrowser { name: String },
    AgentBrowser { name: String },
}

impl Mode {
    fn args(&self) -> Vec<String> {
        match self {
            Mode::XApp => vec!["--x-app".into()],
            Mode::PsyopRead { name } => vec!["--psyop-read".into(), name.clone()],
            Mode::PsyopAuthorize { name } => vec!["--psyop-authorize".into(), name.clone()],
            Mode::AgentAuthorize { name } => vec!["--agent-authorize".into(), name.clone()],
            Mode::PsyopBrowser { name } => vec!["--psyop-browser".into(), name.clone()],
            Mode::AgentBrowser { name } => vec!["--agent-browser".into(), name.clone()],
        }
    }
}

/// Spawn the browser. `state_dir` is the state root (the same
/// `OBJECTIVEAI_STATE_DIR` value the SDK's `auth_json` /
/// `x_app_credentials` modules root at) — the browser builds
/// `<state_dir>/browser/...` underneath it.
///
/// * `pipe_stdin` — pipe the child's stdin so the caller can send
///   [`psychological_operations_sdk::browser::request::Request`]s
///   (notably `Shutdown` from the `login` flow). When `false`, stdin
///   is inherited so the operator's terminal stays interactive
///   (`psyops browse`).
/// * `pipe_stdout` — pipe the child's stdout so the caller can
///   stream [`psychological_operations_sdk::browser::output::Output`]
///   events (needed for both `psyops browse` to consume `tweet_id`s
///   and the `login` flow to watch for `AuthorizeSucceeded` /
///   `AuthorizeFailed`).
pub fn spawn(
    binary: &Path,
    state_dir: &Path,
    mode: Mode,
    pipe_stdin: bool,
    pipe_stdout: bool,
) -> Result<Child, Error> {
    let mut cmd = Command::new(binary);
    cmd.arg("--state-dir").arg(state_dir);
    cmd.args(mode.args());
    if pipe_stdin {
        cmd.stdin(Stdio::piped());
    }
    if pipe_stdout {
        cmd.stdout(Stdio::piped());
    }
    cmd.spawn()
        .map_err(|e| Error::Other(format!("failed to spawn browser: {e}")))
}
