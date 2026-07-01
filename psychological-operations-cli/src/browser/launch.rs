//! Spawn the psychological-operations-browser binary (from the plugin's
//! `OBJECTIVEAI_BIN_DIR`, via [`super::browser_binary`]) in a given mode.
//! Caller has already resolved the mode (and any psyop/agent name).

use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

use crate::error::Error;

/// CLI-side mirror of the browser's `--<mode>` flag set. We don't
/// reach for the SDK's [`psychological_operations_sdk::browser::mode::Mode`]
/// here because the SDK enum carries no inherent CLI representation
/// (it's used renderer-side for `__PSYOPS_MODE` injection); the
/// browser's clap layer in `args.rs` is the source of truth for the
/// flag spellings and we mirror it.
pub enum Mode {
    XApp,
    AgentRead {
        name: String,
    },
    AgentAuthorize {
        name: String,
        /// X-App OAuth client creds (CLI reads them from the captured X-App)
        /// — the browser needs them for the PKCE token exchange.
        client_id: String,
        client_secret: String,
    },
    AgentBrowser {
        name: String,
    },
    /// Reply/quote delivery for one `agent`: `items_json` is an inline JSON
    /// array of [`psychological_operations_sdk::browser::deliver::DeliverItem`]
    /// (each `{tweet_id, content, kind}` — the agent rides on the flag).
    AgentDeliver {
        agent: String,
        items_json: String,
    },
    /// Discord bot-creation wizard for one agent (`name`).
    DiscordLogin {
        name: String,
    },
    /// Master Twitch application setup (scrape client_id + client_secret off
    /// the dev console).
    TwitchApp,
    /// Per-agent Twitch OAuth authorize for one agent (`name`). The browser
    /// drives the OAuth code flow with the master app's creds (the CLI reads
    /// them from the captured Twitch app).
    TwitchAuthorize {
        name: String,
        client_id: String,
        client_secret: String,
    },
}

impl Mode {
    fn args(&self) -> Vec<String> {
        match self {
            Mode::XApp => vec!["--x-app".into()],
            Mode::AgentRead { name } => vec!["--agent-read".into(), name.clone()],
            Mode::AgentAuthorize {
                name,
                client_id,
                client_secret,
            } => vec![
                "--agent-authorize".into(),
                name.clone(),
                "--x-app-client-id".into(),
                client_id.clone(),
                "--x-app-client-secret".into(),
                client_secret.clone(),
            ],
            Mode::AgentBrowser { name } => vec!["--agent-browser".into(), name.clone()],
            Mode::AgentDeliver { agent, items_json } => vec![
                "--agent-deliver".into(),
                agent.clone(),
                "--agent-deliver-items".into(),
                items_json.clone(),
            ],
            Mode::DiscordLogin { name } => vec!["--discord-login".into(), name.clone()],
            Mode::TwitchApp => vec!["--twitch-app".into()],
            Mode::TwitchAuthorize {
                name,
                client_id,
                client_secret,
            } => vec![
                "--twitch-authorize".into(),
                name.clone(),
                "--twitch-client-id".into(),
                client_id.clone(),
                "--twitch-client-secret".into(),
                client_secret.clone(),
            ],
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
    // Reaper-spawn so the OS kills the browser (and its CEF subprocess tree
    // on Windows) if we die. It sets `kill_on_drop` itself — don't double it.
    objectiveai_sdk::subprocess_reaper::spawn(&mut cmd)
        .map_err(|e| Error::Other(format!("failed to spawn browser: {e}")))
}
