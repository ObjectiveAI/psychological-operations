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
use indexmap::IndexMap;
use objectiveai_sdk::agent::{
    ClientObjectiveaiMcp, ClientObjectiveaiMcpPluginEntry, ClientObjectiveaiMcpPluginMcpServer,
    InlineAgentBase, InlineAgentBaseWithFallbacks,
    InlineAgentBaseWithFallbacksOrRemoteCommitOptional, mock,
};
use objectiveai_sdk::cli::command::agents::instances::list as instances_list;
use objectiveai_sdk::cli::command::agents::logs::read::{
    all as logs_read_all, id as logs_read_id, pending as logs_read_pending,
};
use objectiveai_sdk::cli::command::agents::message::RequestMessage;
use objectiveai_sdk::cli::command::agents::queue::deliver as queue_deliver;
use objectiveai_sdk::cli::command::agents::queue::read::pending as queue_read_pending;
use objectiveai_sdk::cli::command::agents::selector::AgentSelector;
use objectiveai_sdk::cli::command::agents::spawn as agents_spawn;
use objectiveai_sdk::cli::command::agents::tags::apply as tags_apply;
use objectiveai_sdk::cli::command::binary::{BinaryExecutor, Error as ExecError};
use objectiveai_sdk::cli::command::plugins::run as plugins_run;
use objectiveai_sdk::cli::command::{CommandExecutor, CommandRequest, CommandResponse};
use objectiveai_sdk::functions::executions::request::Strategy;
use objectiveai_sdk::functions::{
    FullInlineFunctionOrRemoteCommitOptional, InlineProfileOrRemoteCommitOptional,
};
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::cli::destinations::{DeliverySummary, Destination};
use psychological_operations_sdk::cli::psyops::{
    PsyOp, PsyopEntry, PublishedPsyop, Query, SearchEndpoint, SortBy, Stage, StageBase,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The objectiveai `agents` `--target` selector (`me` / `tag=…` /
/// `instance=…`), re-exported so host-command tests can name it.
pub use objectiveai_sdk::cli::command::agents::logs::read::all::Target;

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

    /// `targets add (--global | --psyop <name>) <json>` — the destination
    /// is a TYPED [`Destination`], serialized to the JSON the CLI expects.
    pub async fn targets_add(&self, sel: Selector<'_>, dest: &Destination) -> RunResult {
        let json = serde_json::to_string(dest).expect("destination serializes");
        let mut v = vec!["targets".into(), "add".into()];
        sel.append(&mut v);
        v.push(json);
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

    // ── agents: notify (plugin) ─────────────────────────────────────

    /// `agents notify` (PLUGIN, no args) — park an objectiveai
    /// notification for every agent with queued tweets.
    pub async fn agents_notify(&self) -> RunResult {
        self.dispatch(vec!["agents".into(), "notify".into()]).await
    }

    /// `agents enqueue` (PLUGIN) — park `tweet_id` (with `message`) on
    /// `agent`'s psychological-operations queue.
    pub async fn agents_enqueue(
        &self,
        agent: Agent<'_>,
        tweet_id: &str,
        message: &str,
    ) -> RunResult {
        let mut v = vec!["agents".into(), "enqueue".into()];
        agent.append(&mut v);
        v.push("--tweet-id".into());
        v.push(tweet_id.to_string());
        v.push("--message".into());
        v.push(message.to_string());
        self.dispatch(v).await
    }

    // ── agents: objectiveai HOST root commands ──────────────────────
    //
    // These drive the objectiveai host directly (NOT the psychological-
    // operations plugin): the same executor spawns `objectiveai agents
    // …` via each typed Request's `into_command`. Distinct from the
    // `agents_quota_*` / `agents_notify` plugin methods above.

    /// `agents tags apply` (HOST) — register `tag` bound to a fresh inline
    /// agent (`inner`). Creates a GROUPED tag carrying the agent spec.
    pub async fn agents_tags_apply_inline(
        &self,
        tag: &str,
        inner: InlineAgentBase,
    ) -> HostResult<tags_apply::Response> {
        let agent_spec = InlineAgentBaseWithFallbacksOrRemoteCommitOptional::AgentBase(
            InlineAgentBaseWithFallbacks { inner, fallbacks: None },
        );
        let req = tags_apply::Request {
            path_type: tags_apply::Path::AgentsTagsApply,
            name: tag.to_string(),
            target: tags_apply::Target::Agent {
                agent_spec,
                parent_agent_instance_hierarchy: None,
            },
            base: Default::default(),
        };
        self.host(req).await
    }

    /// Convenience: register `tag` bound to a default (no-op) inline mock
    /// agent (deterministic, no upstream LLM, no tools).
    pub async fn agents_tags_apply_mock(&self, tag: &str) -> HostResult<tags_apply::Response> {
        self.agents_tags_apply_inline(tag, InlineAgentBase::Mock(mock::AgentBase::default()))
            .await
    }

    /// `agents spawn` (HOST) with streaming on and an EMPTY initial
    /// message — spawns the `tag`'s agent, drains its queued messages, and
    /// streams every chunk to completion (a synchronous barrier).
    pub async fn agents_spawn_stream(&self, tag: &str) -> HostResult<agents_spawn::ResponseItem> {
        let req = agents_spawn::Request {
            path_type: agents_spawn::Path::AgentsSpawn,
            message: RequestMessage::Simple(String::new()),
            agent: AgentSelector::Tag { agent_tag: tag.to_string() },
            dangerous_advanced: Some(agents_spawn::RequestDangerousAdvanced {
                stream: Some(true),
                seed: Some(42),
                skip_lock: None,
            }),
            base: Default::default(),
        };
        self.host(req).await
    }

    /// `agents queue read pending` (HOST) — pending objectiveai
    /// `message_queue` rows under `targets` (an empty item list ⇒ the
    /// queue is empty).
    pub async fn agents_queue_read_pending(
        &self,
        targets: Vec<Target>,
    ) -> HostResult<queue_read_pending::ResponseItem> {
        let req = queue_read_pending::Request {
            path_type: queue_read_pending::Path::AgentsQueueReadPending,
            targets,
            after_id: None,
            limit: None,
            base: Default::default(),
        };
        self.host(req).await
    }

    /// `agents queue deliver` (HOST) with stream mode on
    /// (`stream_spawns: true`) — wins each pending agent's lock, spawns
    /// + runs it in-process, and streams to `AllAgentsActive`. A
    /// synchronous delivery barrier: when this returns, every spawn has
    /// finished.
    pub async fn agents_queue_deliver_stream(&self) -> HostResult<queue_deliver::ResponseItem> {
        let req = queue_deliver::Request {
            path_type: queue_deliver::Path::AgentsQueueDeliver,
            dangerous_advanced: Some(queue_deliver::RequestDangerousAdvanced {
                stream_spawns: Some(true),
            }),
            base: Default::default(),
        };
        self.host(req).await
    }

    /// `agents instances list` (HOST) — per-agent aggregates for each
    /// instance under `targets`.
    pub async fn agents_instances_list(
        &self,
        targets: Vec<Target>,
    ) -> HostResult<instances_list::ResponseItem> {
        let req = instances_list::Request {
            path_type: instances_list::Path::AgentsInstancesList,
            targets,
            base: Default::default(),
        };
        self.host(req).await
    }

    /// `agents logs read pending` (HOST) — pending child log rows under
    /// `targets`.
    pub async fn agents_logs_read_pending(
        &self,
        targets: Vec<Target>,
    ) -> HostResult<logs_read_all::ResponseItem> {
        let req = logs_read_pending::Request {
            path_type: logs_read_pending::Path::AgentsLogsReadPending,
            targets,
            after_id: None,
            limit: None,
            base: Default::default(),
        };
        self.host(req).await
    }

    /// `agents logs read all` (HOST) — every log row under `targets`
    /// (delivered or not). Target a spawned tag agent with
    /// `Target::Tag { agent_tag }` (the agent's own logs live under its
    /// instance, not the caller's `me`).
    pub async fn agents_logs_read_all(
        &self,
        targets: Vec<Target>,
    ) -> HostResult<logs_read_all::ResponseItem> {
        let req = logs_read_all::Request {
            path_type: logs_read_all::Path::AgentsLogsReadAll,
            targets,
            after_id: None,
            limit: None,
            base: Default::default(),
        };
        self.host(req).await
    }

    /// `agents logs read id <id>` (HOST) — the single log row at
    /// `logs.messages."index" == id`.
    pub async fn agents_logs_read_id(&self, id: i64) -> HostResult<logs_read_id::Response> {
        let req = logs_read_id::Request {
            path_type: logs_read_id::Path::AgentsLogsReadId,
            id,
            base: Default::default(),
        };
        self.host(req).await
    }

    /// Core: run a typed objectiveai HOST `Request` through the executor,
    /// collecting the typed stream into a [`HostResult`]. Panics on a
    /// harness/infra failure (spawn, IO, undecodable line).
    async fn host<R, T>(&self, req: R) -> HostResult<T>
    where
        R: CommandRequest + Send,
        T: CommandResponse + Serialize + DeserializeOwned + Send + 'static,
    {
        let label = req.into_command().join(" ");
        let mut stream = self
            .executor
            .execute::<_, T>(req, None)
            .await
            .unwrap_or_else(|e| panic!("[{}] execute `{label}`: {e}", self.state));
        let mut items = Vec::new();
        let mut errors = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(t) => items.push(t),
                Err(ExecError::Cli(e)) => errors.push(e),
                Err(other) => panic!("[{}] `{label}` harness error: {other}", self.state),
            }
        }
        HostResult { items, errors }
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

/// Build a minimal valid [`PsyOp`] that ingests via one deterministic
/// mock-X search `query` (no `for_you` — tests must never configure it,
/// since for-you collection drives the real CEF browser), with the given
/// scoring `stages` (empty = no scoring → max-score survivors).
pub fn query_psyop(query: &str, stages: Vec<Stage>) -> PsyOp {
    PsyOp {
        queries: Some(vec![Query {
            query: query.to_string(),
            endpoint: SearchEndpoint::Recent,
            priority: None,
            filter: None,
        }]),
        for_you: None,
        interval: "1h".to_string(),
        min_posts: 2,
        max_posts: 10,
        sort: SortBy::Newest,
        query_when_for_you_queued: true,
        stages: if stages.is_empty() { None } else { Some(stages) },
    }
}

/// An inline **mock** agent scripted to make ONE deterministic tool call to
/// the psychological-operations x-api MCP tool `mark_handled`
/// (`{"account": <account>, "tweet_ids": [...]}`), then close out with a
/// content turn. It wires `client_objectiveai_mcp` so the plugin's `x-api`
/// MCP server is exposed in **full** mode (`mark_handled` is hidden in
/// readonly). The tool name is the proxy-prefixed `<serverInfo.name>_<tool>`
/// = `psychological-operations-x-api_mark_handled`.
pub fn mark_handled_mock_agent(account: &str, tweet_ids: &[&str]) -> InlineAgentBase {
    let arguments =
        serde_json::json!({ "account": account, "tweet_ids": tweet_ids }).to_string();
    let mut base = mock::AgentBase::default();
    base.calls = Some(vec![
        // Turn 1: the deterministic tool call.
        mock::Call {
            tool_calls: vec![mock::CallToolCall {
                name: "psychological-operations-x-api_mark_handled".to_string(),
                arguments,
            }],
            content: String::new(),
        },
        // Turn 2: close out with content (no tool calls) so the agent
        // completion finishes after the tool result comes back.
        mock::Call { tool_calls: Vec::new(), content: "done".to_string() },
    ]);
    base.client_objectiveai_mcp = Some(ClientObjectiveaiMcp {
        objectiveai: None,
        plugins: vec![ClientObjectiveaiMcpPluginEntry {
            owner: OWNER.to_string(),
            name: NAME.to_string(),
            version: VERSION.to_string(),
            // Don't surface the plugin's own command tools — only bring up
            // its declared `x-api` MCP server (which carries mark_handled).
            executable: false,
            mcp_servers: Some(vec![ClientObjectiveaiMcpPluginMcpServer {
                name: "x-api".to_string(),
                // Forwarded as the per-request `X-OBJECTIVEAI-ARGUMENTS`
                // header → the x-api session reads `mode` (FULL, so
                // mark_handled is visible) and the REQUIRED `account`.
                // The session's agent identity still comes from the
                // agent's `X-OBJECTIVEAI-AGENT-INSTANCE-HIERARCHY` header
                // (an `agent` arg would make the host launch
                // `mcp x-api begin --agent …`, which isn't a valid flag).
                arguments: Some(IndexMap::from([
                    ("mode".to_string(), Some("full".to_string())),
                    ("account".to_string(), Some(account.to_string())),
                ])),
            }]),
        }],
        tools: Vec::new(),
    });
    InlineAgentBase::Mock(base)
}

/// A `function` scoring stage backed by an INLINE `vector.function` + an
/// INLINE profile with a deterministic **mock** agent — no fixtures, no
/// remote. (`remote:mock` named functions don't exist in the clean-slate
/// model and aren't supported by the host's `functions get`.) The function
/// ranks the ingested posts in one `vector.completion` task whose
/// `responses` are built per-item via Starlark (so the vote vector length
/// matches the post count) and whose `task_output_l1_normalized` output
/// yields the per-post score array the psyop pipeline extracts. Pass
/// `top_logprobs` to exercise a logprobs swarm.
pub fn mock_function_stage(top_logprobs: Option<u64>) -> Stage {
    let function: FullInlineFunctionOrRemoteCommitOptional = serde_json::from_value(
        serde_json::json!({
            "type": "vector.function",
            "tasks": [{
                "type": "vector.completion",
                "messages": [{ "role": "user", "content": "Rank these posts by quality." }],
                "responses": {
                    "$starlark": "[[{'type': 'text', 'text': item['text']}] for item in input['items']]"
                },
                "output": { "$special": "task_output_l1_normalized" }
            }]
        }),
    )
    .expect("inline vector.function deserializes");

    let agent = match top_logprobs {
        Some(n) => serde_json::json!({
            "upstream": "mock", "output_mode": "instruction", "top_logprobs": n
        }),
        None => serde_json::json!({ "upstream": "mock", "output_mode": "instruction" }),
    };
    let profile: InlineProfileOrRemoteCommitOptional = serde_json::from_value(serde_json::json!({
        "agents": [agent],
        "weights": [1.0]
    }))
    .expect("inline profile deserializes");

    Stage::Function {
        base: StageBase { output_top: None },
        function,
        profile,
        strategy: Strategy::Default,
        invert: false,
        images: false,
        videos: false,
        output_threshold: None,
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

/// Classified output of one objectiveai HOST command — the typed items
/// of its stream plus any error frames. (Host commands yield a single
/// typed response stream, unlike plugin commands whose mixed stream is
/// split across the fields of [`RunResult`].)
pub struct HostResult<T> {
    /// The decoded response items, in stream order.
    pub items: Vec<T>,
    /// Error frames surfaced by the host.
    pub errors: Vec<objectiveai_sdk::cli::Error>,
}

impl<T> HostResult<T> {
    /// Assert no error frame was emitted; returns `self` for chaining.
    pub fn assert_no_errors(&self) -> &Self {
        assert!(
            self.errors.is_empty(),
            "expected no host errors, got: {:?}",
            self.errors,
        );
        self
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

    /// How many mid-stream events have `event == name` (e.g.
    /// `"stage_begin"`, `"query_complete"`, `"target_delivered"`).
    pub fn event_count(&self, name: &str) -> usize {
        self.events
            .iter()
            .filter(|e| e.get("event").and_then(|v| v.as_str()) == Some(name))
            .count()
    }

    /// True iff at least one mid-stream event has `event == name`.
    pub fn has_event(&self, name: &str) -> bool {
        self.event_count(name) > 0
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
