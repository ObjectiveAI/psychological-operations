//! `mcp begin` supervisor — probe-or-spawn the shared X-API MCP
//! and return its URL plus the per-session headers a client must
//! send to bind its session to the requested `(agent, mode)`.
//!
//! State lives at `${TMPDIR}/psychological-operations-x-api-mcp/state.json` —
//! ONE process per host now, not per (agent, mode). The MCP itself
//! multiplexes sessions by reading `X-PSYOP-X-API-AGENT` /
//! `X-PSYOP-X-API-MODE` headers off the initialize POST.
//!
//! Probe: if state exists, look up the recorded PID via sysinfo and
//! match the process name; if alive, return the recorded URL.
//! Otherwise spawn a detached child with `stdin/stdout=null`,
//! `stderr=piped`, parse `"listening on <addr>"` off stderr (same line
//! `psychological-operations-x-api-mcp/src/run.rs::run` prints once
//! its TCP listener is bound on `:0`), persist new state, drop the
//! child to detach (Windows: `CREATE_NO_WINDOW | DETACHED_PROCESS`;
//! Unix: kernel re-parents to init).
//!
//! Silent respawn on stale state.json: no notification, both probe-hit
//! and fresh-spawn paths emit the same `Output::Mcp` shape.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::commands::mcp::Mode;
use crate::error::Error;
use crate::mcp::embed::{self, X_API_MCP_BINARY_NAME};

/// Must stay byte-identical to
/// `psychological-operations-x-api-mcp/src/x_api/session.rs::HEADER_AGENT`.
/// Duplicated rather than imported because the MCP crate is
/// embedded via `build.rs` include_bytes, not linked.
const HEADER_AGENT: &str = "X-PSYOP-X-API-AGENT";

/// Must stay byte-identical to
/// `psychological-operations-x-api-mcp/src/x_api/session.rs::HEADER_MODE`.
const HEADER_MODE: &str = "X-PSYOP-X-API-MODE";

#[derive(Serialize, Deserialize)]
struct State {
    pid: u32,
    url: String,
    /// The process name we expect `sysinfo` to report. Defeats PID
    /// recycling — if the recorded PID is now owned by something
    /// else, the name won't match and we respawn.
    exe_name: String,
}

pub async fn run(
    agent: &str,
    mode: Mode,
    cache_max_size: u64,
    cache_ttl: u64,
    cfg: &crate::run::Config,
) -> Result<crate::Output, Error> {
    let state_dir = std::env::temp_dir().join("psychological-operations-x-api-mcp");
    let state_file = state_dir.join("state.json");

    let url = match read_state(&state_file).await? {
        Some(state) if is_alive(&state) => state.url,
        _ => {
            let binary = embed::ensure_extracted().await?;
            let config_base_dir = cfg.objectiveai_base_dir();
            let url = spawn_and_wait(&binary, &config_base_dir, cache_max_size, cache_ttl).await?;

            tokio::fs::create_dir_all(&state_dir).await?;
            let pid = pid_for_url(&binary)?;
            let state = State {
                pid,
                url: url.clone(),
                exe_name: X_API_MCP_BINARY_NAME.to_string(),
            };
            let bytes = serde_json::to_vec(&state)?;
            tokio::fs::write(&state_file, bytes).await?;
            url
        }
    };

    let mut headers = BTreeMap::new();
    headers.insert(HEADER_AGENT.to_string(), agent.to_string());
    headers.insert(HEADER_MODE.to_string(), mode.as_arg_str().to_string());

    Ok(crate::Output::Mcp { url, headers })
}

async fn read_state(state_file: &Path) -> Result<Option<State>, Error> {
    match tokio::fs::read(state_file).await {
        Ok(bytes) => Ok(serde_json::from_slice::<State>(&bytes).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn is_alive(state: &State) -> bool {
    let mut sys = System::new();
    let pid = Pid::from_u32(state.pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid)
        .map(|p| matches_exe(p.name().to_string_lossy().as_ref(), &state.exe_name))
        .unwrap_or(false)
}

/// We spawn the child and detach (drop) it inside `spawn_and_wait`,
/// so the `child.id()` from that call doesn't survive to here. Re-derive
/// the PID by scanning sysinfo for our exe name — at this point exactly
/// one freshly-spawned process should match (and PID recycling is
/// defeated downstream by the name match in `is_alive`).
fn pid_for_url(binary: &Path) -> Result<u32, Error> {
    let target = binary
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| X_API_MCP_BINARY_NAME.to_string());

    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing(),
    );
    let mut pids: Vec<u32> = sys
        .processes()
        .values()
        .filter(|p| matches_exe(p.name().to_string_lossy().as_ref(), &target))
        .map(|p| p.pid().as_u32())
        .collect();
    // Most-recently-spawned wins on the rare collision (e.g. another
    // agent's MCP was just spawned). The supervisor flow is single-
    // threaded per agent, so this is best-effort.
    pids.sort_unstable();
    pids.pop().ok_or_else(|| {
        Error::Other(format!(
            "spawned MCP {} not found in process table",
            X_API_MCP_BINARY_NAME
        ))
    })
}

/// Spawn the X-API MCP detached and read its `"listening on <addr>"`
/// line off stderr. Returns `http://<addr>`.
async fn spawn_and_wait(
    binary: &Path,
    config_base_dir: &Path,
    cache_max_size: u64,
    cache_ttl: u64,
) -> Result<String, Error> {
    let mut cmd = Command::new(binary);
    cmd.arg("--config-base-dir").arg(config_base_dir)
        .arg("--cache-max-size").arg(cache_max_size.to_string())
        .arg("--cache-ttl").arg(cache_ttl.to_string())
        .arg("--port").arg("0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW (0x08000000) | DETACHED_PROCESS (0x00000008).
        cmd.creation_flags(0x0800_0008);
    }

    let mut child = cmd.spawn().map_err(|e| {
        Error::Other(format!(
            "spawn {}: {}",
            X_API_MCP_BINARY_NAME, e
        ))
    })?;

    let stderr = child.stderr.take().ok_or_else(|| {
        Error::Other(format!(
            "{} stderr was not piped",
            X_API_MCP_BINARY_NAME
        ))
    })?;

    let mut reader = BufReader::new(stderr).lines();
    let mut listening_line: Option<String> = None;
    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if line.to_ascii_lowercase().contains("listening on ") {
                            listening_line = Some(line);
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            status = child.wait() => {
                let _ = status;
                break;
            }
        }
    }

    let line = listening_line.ok_or_else(|| {
        Error::Other(format!(
            "{} exited before announcing its listening address",
            X_API_MCP_BINARY_NAME
        ))
    })?;

    // Drain remaining stderr fire-and-forget so the child's pipe
    // buffer doesn't fill after we detach. The task ends when the
    // child closes stderr (or the cli exits).
    tokio::spawn(async move { while let Ok(Some(_)) = reader.next_line().await {} });

    // Drop the Child so it's detached. tokio's `Child` does not kill
    // on drop by default — on Unix the kernel re-parents to init when
    // the cli exits; on Windows the parent handle is released and the
    // creation flags above keep the child off the parent console.
    drop(child);

    let addr = parse_addr(&line).ok_or_else(|| {
        Error::Other(format!("could not parse listening address from line: {line:?}"))
    })?;
    Ok(format!("http://{addr}"))
}

/// `"listening on 127.0.0.1:54321"` → `Some("127.0.0.1:54321")`.
fn parse_addr(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("listening on ")?;
    let rest = &line[idx + "listening on ".len()..];
    let addr = rest.split_whitespace().next()?.trim();
    if addr.is_empty() { None } else { Some(addr.to_string()) }
}

/// Case-insensitive exe-name compare; strips `.exe` so the same target
/// string works on Linux + macOS + Windows.
fn matches_exe(observed: &str, target: &str) -> bool {
    let trim = |s: &str| {
        s.strip_suffix(".exe")
            .or_else(|| s.strip_suffix(".EXE"))
            .unwrap_or(s)
            .to_ascii_lowercase()
    };
    trim(observed) == trim(target)
}
