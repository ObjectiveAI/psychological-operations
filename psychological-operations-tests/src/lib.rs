//! Integration-test harness for psychological-operations.
//!
//! Every test drives the prebuilt `objectiveai` binary in the repo's
//! `.objectiveai/` (populated by `test-prepare.sh`) through the SDK
//! [`BinaryExecutor`]. Each test gets a clean, isolated state by setting
//! `OBJECTIVEAI_STATE` to its own name; all tests share one
//! `OBJECTIVEAI_DIR` and the same binary, and the host auto-bootstraps a
//! fresh per-state postgres on first command. Setup, actions, AND
//! assertions all go through the executor — **no test reads the
//! filesystem**.
//!
//! Command output is deserialized two layers deep:
//!   outer `objectiveai_sdk::cli::command::plugins::run::ResponseItem`
//!   → inner `psychological_operations_sdk::cli::Output`
//! and asserted on partially (specific fields), not full-text snapshotted.

use std::path::PathBuf;

use futures::StreamExt;
use objectiveai_sdk::cli::command::CommandExecutor;
use objectiveai_sdk::cli::command::binary::{BinaryExecutor, Error as ExecError};
use objectiveai_sdk::cli::command::plugins::run as plugins_run;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::cli::destinations::{DeliverySummary, Destination};
use psychological_operations_sdk::cli::psyops::{PsyOp, PsyopEntry, PublishedPsyop};
use serde_json::Value;

/// The plugin coordinate installed under `.objectiveai/bin/plugins/`.
const OWNER: &str = "objectiveai";
const NAME: &str = "psychological-operations";
const VERSION: &str = "1.0.0";

/// Absolute path to the repo's `.objectiveai/` — the executor's
/// `OBJECTIVEAI_DIR`. Derived from the crate manifest dir at compile
/// time (a path constant, not a runtime filesystem read).
fn objectiveai_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent (the workspace root)")
        .join(".objectiveai")
}

/// The prebuilt objectiveai host binary (`<dir>/bin/objectiveai[.exe]`).
fn objectiveai_binary() -> PathBuf {
    let name = if cfg!(windows) { "objectiveai.exe" } else { "objectiveai" };
    objectiveai_dir().join("bin").join(name)
}

/// A handle that runs psychological-operations plugin commands against
/// one isolated objectiveai state, via the SDK [`BinaryExecutor`].
pub struct Plugin {
    executor: BinaryExecutor,
    state: String,
}

impl Plugin {
    /// Build a handle for a test. `state` (pass the test's own name)
    /// becomes `OBJECTIVEAI_STATE`, giving the test a fresh isolated
    /// state the host bootstraps on first command. Mock mode is on by
    /// default (deterministic, no outbound X); use [`Self::with_mock`]
    /// to vary it.
    pub fn new(state: &str) -> Self {
        Self::with_mock(state, true)
    }

    /// Like [`Self::new`] but with an explicit mock setting.
    pub fn with_mock(state: &str, mock: bool) -> Self {
        let dir = objectiveai_dir();
        let executor = BinaryExecutor::new(Some(dir.clone()))
            .env("OBJECTIVEAI_DIR", dir.to_string_lossy().into_owned())
            .env("OBJECTIVEAI_STATE", state)
            .env(
                "PSYCHOLOGICAL_OPERATIONS_MOCK",
                if mock { "true" } else { "false" },
            )
            // Tear the host child down when the response stream is
            // dropped, so a panicking assertion mid-stream doesn't leave
            // the process running.
            .kill_on_drop(true);
        Self { executor, state: state.to_string() }
    }

    // ── psyops ──────────────────────────────────────────────────────

    /// `psyops publish` from a TYPED definition: serialize the [`PsyOp`]
    /// struct to the `--psyop-inline` JSON the CLI expects, so the
    /// definition is compile-time-checked.
    pub async fn psyops_publish(&self, name: &str, psyop: &PsyOp) -> RunResult {
        let json = serde_json::to_string(psyop).expect("psyop serializes");
        self.dispatch(vec![
            "psyops".into(),
            "publish".into(),
            "--name".into(),
            name.into(),
            "--psyop-inline".into(),
            json,
        ])
        .await
    }

    /// `psyops list`.
    pub async fn psyops_list(&self) -> RunResult {
        self.dispatch(vec!["psyops".into(), "list".into()]).await
    }

    /// `psyops get <name>`.
    pub async fn psyops_get(&self, name: &str) -> RunResult {
        self.dispatch(vec!["psyops".into(), "get".into(), name.into()]).await
    }

    /// `psyops enable <name>`.
    pub async fn psyops_enable(&self, name: &str) -> RunResult {
        self.dispatch(vec!["psyops".into(), "enable".into(), name.into()]).await
    }

    /// `psyops disable <name>`.
    pub async fn psyops_disable(&self, name: &str) -> RunResult {
        self.dispatch(vec!["psyops".into(), "disable".into(), name.into()]).await
    }

    /// `psyops run [--name X]… [--seed N]`. An empty `names` runs every
    /// interval-eligible psyop.
    pub async fn psyops_run(&self, names: &[&str], seed: Option<i64>) -> RunResult {
        let mut v = vec!["psyops".into(), "run".into()];
        for n in names {
            v.push("--name".into());
            v.push((*n).to_string());
        }
        if let Some(s) = seed {
            v.push("--seed".into());
            v.push(s.to_string());
        }
        self.dispatch(v).await
    }

    // ── targets ─────────────────────────────────────────────────────

    /// `targets list (--global | --psyop <name>)`.
    pub async fn targets_list(&self, sel: Selector<'_>) -> RunResult {
        let mut v = vec!["targets".into(), "list".into()];
        sel.append(&mut v);
        self.dispatch(v).await
    }

    /// `targets deliver (--global | --psyop <name>)`.
    pub async fn targets_deliver(&self, sel: Selector<'_>) -> RunResult {
        let mut v = vec!["targets".into(), "deliver".into()];
        sel.append(&mut v);
        self.dispatch(v).await
    }

    // ── agents quota ────────────────────────────────────────────────

    /// `agents quota limit set <agent> (--read|--write) <value>`.
    pub async fn agents_quota_limit_set(&self, agent: Agent<'_>, dir: Dir, value: u64) -> RunResult {
        let mut v = vec!["agents".into(), "quota".into(), "limit".into(), "set".into()];
        agent.append(&mut v);
        v.push(dir.flag().into());
        v.push(value.to_string());
        self.dispatch(v).await
    }

    /// `agents quota limit get <agent> (--read|--write)`.
    pub async fn agents_quota_limit_get(&self, agent: Agent<'_>, dir: Dir) -> RunResult {
        let mut v = vec!["agents".into(), "quota".into(), "limit".into(), "get".into()];
        agent.append(&mut v);
        v.push(dir.flag().into());
        self.dispatch(v).await
    }

    /// `agents quota interval set <agent> (--read|--write) <humantime>`.
    pub async fn agents_quota_interval_set(&self, agent: Agent<'_>, dir: Dir, interval: &str) -> RunResult {
        let mut v = vec!["agents".into(), "quota".into(), "interval".into(), "set".into()];
        agent.append(&mut v);
        v.push(dir.flag().into());
        v.push(interval.to_string());
        self.dispatch(v).await
    }

    /// `agents quota interval get <agent> (--read|--write)`.
    pub async fn agents_quota_interval_get(&self, agent: Agent<'_>, dir: Dir) -> RunResult {
        let mut v = vec!["agents".into(), "quota".into(), "interval".into(), "get".into()];
        agent.append(&mut v);
        v.push(dir.flag().into());
        self.dispatch(v).await
    }

    /// `agents quota tool set <agent> <tool> <cost>` — a tool's direction
    /// is intrinsic, so no `--read/--write`.
    pub async fn agents_quota_tool_set(&self, agent: Agent<'_>, tool: &str, cost: u64) -> RunResult {
        let mut v = vec!["agents".into(), "quota".into(), "tool".into(), "set".into()];
        agent.append(&mut v);
        v.push(tool.to_string());
        v.push(cost.to_string());
        self.dispatch(v).await
    }

    /// `agents quota tool get <agent> <tool>`.
    pub async fn agents_quota_tool_get(&self, agent: Agent<'_>, tool: &str) -> RunResult {
        let mut v = vec!["agents".into(), "quota".into(), "tool".into(), "get".into()];
        agent.append(&mut v);
        v.push(tool.to_string());
        self.dispatch(v).await
    }

    // ── escape hatch + core ─────────────────────────────────────────

    /// Run an arbitrary plugin command (the plugin's own argv) for
    /// commands without a typed wrapper yet. Prefer the command-named
    /// methods above.
    pub async fn cli(&self, args: &[&str]) -> RunResult {
        self.dispatch(args.iter().map(|s| s.to_string()).collect()).await
    }

    /// Core: run `plugins run …` for this plugin with `args` (the
    /// plugin's argv) through the executor, classifying the two-layer
    /// response. Panics on a harness/infra failure (spawn, IO,
    /// undecodable line); those aren't the behavior under test.
    async fn dispatch(&self, args: Vec<String>) -> RunResult {
        let label = args.join(" ");
        let request = plugins_run::Request {
            path_type: plugins_run::Path::PluginsRun,
            owner: OWNER.to_string(),
            name: NAME.to_string(),
            version: VERSION.to_string(),
            args,
            base: Default::default(),
        };
        let mut stream = self
            .executor
            .execute::<_, plugins_run::ResponseItem>(request, None)
            .await
            .unwrap_or_else(|e| panic!("[{}] execute `{label}`: {e}", self.state));

        let mut result = RunResult::default();
        while let Some(item) = stream.next().await {
            match item {
                Ok(plugins_run::ResponseItem::Notification(value)) => {
                    // Inner layer: a terminal psyops `Output`, else a raw
                    // mid-stream event/notification (e.g. {"event":...}).
                    match serde_json::from_value::<Output>(value.clone()) {
                        Ok(output) => result.outputs.push(output),
                        Err(_) => result.events.push(value),
                    }
                }
                Ok(plugins_run::ResponseItem::Mcp(mcp)) => result.mcps.push(mcp),
                // `BinaryExecutor`'s `Line<T>` matches `cli::Error` before
                // `ResponseItem`, so error frames usually land in the
                // `Err(Cli)` arm below — handle this one for completeness.
                Ok(plugins_run::ResponseItem::Error(e)) => result.errors.push(e),
                Err(ExecError::Cli(e)) => result.errors.push(e),
                Err(other) => panic!("[{}] `{label}` harness error: {other}", self.state),
            }
        }
        result
    }
}

/// `targets` / `psyops` two-way selector → `--global` or `--psyop <name>`.
pub enum Selector<'a> {
    Global,
    Psyop(&'a str),
}

impl Selector<'_> {
    fn append(&self, v: &mut Vec<String>) {
        match self {
            Selector::Global => v.push("--global".into()),
            Selector::Psyop(n) => {
                v.push("--psyop".into());
                v.push((*n).to_string());
            }
        }
    }
}

/// `agents` agent selector → `--me` / `--agent-tag <t>` /
/// `--agent-instance <i>`.
pub enum Agent<'a> {
    Me,
    Tag(&'a str),
    Instance(&'a str),
}

impl Agent<'_> {
    fn append(&self, v: &mut Vec<String>) {
        match self {
            Agent::Me => v.push("--me".into()),
            Agent::Tag(t) => {
                v.push("--agent-tag".into());
                v.push((*t).to_string());
            }
            Agent::Instance(i) => {
                v.push("--agent-instance".into());
                v.push((*i).to_string());
            }
        }
    }
}

/// Quota direction → `--read` / `--write`.
pub enum Dir {
    Read,
    Write,
}

impl Dir {
    fn flag(&self) -> &'static str {
        match self {
            Dir::Read => "--read",
            Dir::Write => "--write",
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        // Best-effort teardown: kill this state's embedded postgres so
        // the per-state postmaster (which deliberately outlives each
        // command) doesn't leak across the suite. A *sync* subprocess —
        // `Drop` can't `block_on` inside the test's tokio runtime, so
        // this is the one path that doesn't go through the async
        // executor. `db kill --state` kills the current state's db
        // lock-owner; it doesn't bootstrap anything.
        let _ = std::process::Command::new(objectiveai_binary())
            .args(["db", "kill", "--state"])
            .env("OBJECTIVEAI_DIR", objectiveai_dir())
            .env("OBJECTIVEAI_STATE", &self.state)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Classified output of one plugin command — built for partial
/// assertions on specific fields rather than full-text snapshots.
#[derive(Default)]
pub struct RunResult {
    /// Terminal psychological-operations outputs (e.g. `Ok`, `PsyopList`,
    /// `DeliverySummary`).
    pub outputs: Vec<Output>,
    /// Mid-stream items that aren't a terminal `Output` — raw JSON, e.g.
    /// progress events (`{"event":"stage_begin"}`) and the `agents quota
    /// … get` notifications (`{"account":…,"limit":…}`).
    pub events: Vec<Value>,
    /// Error frames surfaced by the host or plugin.
    pub errors: Vec<objectiveai_sdk::cli::Error>,
    /// MCP-URL announcements (only from `mcp x-api begin`).
    pub mcps: Vec<plugins_run::Mcp>,
}

impl RunResult {
    /// Assert no error frame was emitted; returns `self` for chaining.
    pub fn assert_no_errors(&self) -> &Self {
        assert!(
            self.errors.is_empty(),
            "expected no errors, got: {:?}",
            self.errors,
        );
        self
    }

    /// Assert a terminal `Output::Ok` was emitted (and no errors).
    pub fn assert_ok(&self) -> &Self {
        self.assert_no_errors();
        assert!(
            self.outputs.iter().any(|o| matches!(o, Output::Ok)),
            "expected an Output::Ok, got: {:?}",
            self.outputs,
        );
        self
    }

    /// The first `psyops list` result.
    pub fn psyop_list(&self) -> &[PsyopEntry] {
        self.outputs
            .iter()
            .find_map(|o| match o {
                Output::PsyopList(v) => Some(v.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no psyop_list output among {:?}", self.outputs))
    }

    /// The first `psyops publish` result.
    pub fn published(&self) -> &PublishedPsyop {
        self.outputs
            .iter()
            .find_map(|o| match o {
                Output::PublishedPsyop(p) => Some(p),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no published_psyop output among {:?}", self.outputs))
    }

    /// The first `psyops get` result.
    pub fn psyop(&self) -> &PsyOp {
        self.outputs
            .iter()
            .find_map(|o| match o {
                Output::Psyop(p) => Some(p),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no psyop output among {:?}", self.outputs))
    }

    /// The first `targets list` result.
    pub fn destination_list(&self) -> &[Destination] {
        self.outputs
            .iter()
            .find_map(|o| match o {
                Output::DestinationList(v) => Some(v.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no destination_list output among {:?}", self.outputs))
    }

    /// The first `targets deliver` summary.
    pub fn delivery_summary(&self) -> &DeliverySummary {
        self.outputs
            .iter()
            .find_map(|o| match o {
                Output::DeliverySummary(s) => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no delivery_summary output among {:?}", self.outputs))
    }
}
