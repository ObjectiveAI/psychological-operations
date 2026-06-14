//! `agents quota` — per-account, per-tool-call quota configuration.
//!
//! Quota is metered per **account** (the identity a tool acts as, keyed
//! by the same agent name an [`AgentRef`] resolves to) and enforced by
//! the X-API MCP per tool call: each tool is intrinsically a read XOR a
//! write with a configurable cost, charged against the account's
//! read/write budget over a trailing per-direction interval. This subtree
//! reads/writes that config in the db; defaults live in the db layer
//! (read limit 30, write limit 10, interval 1h, per-tool cost 1).
//!
//! - `limit  {get,set} <agent> --read|--write [VALUE]`
//! - `interval {get,set} <agent> --read|--write [HUMANTIME]`
//! - `tool   {get,set} <agent> <TOOL> [COST]` — a tool's direction is
//!   intrinsic, so no `--read/--write` here. `read_queue`, `mark_handled`,
//!   and `list_accounts` are quota-free and absent from the `<TOOL>` enum.

use std::time::Duration;

use clap::{ArgGroup, Args, Subcommand};
use psychological_operations_db::QuotaDirection;
use psychological_operations_sdk::cli::Output;
use psychological_operations_x_api_mcp::ToolName;

use super::agent_ref::AgentRef;
use crate::error::Error;

#[derive(Subcommand)]
pub enum Commands {
    /// The max summed tool-call cost allowed per interval, per direction.
    Limit {
        #[command(subcommand)]
        command: LimitCommands,
    },
    /// The trailing sliding-window length the limit is measured over.
    Interval {
        #[command(subcommand)]
        command: IntervalCommands,
    },
    /// How much one call of a given tool deducts from its direction's
    /// budget.
    Tool {
        #[command(subcommand)]
        command: ToolCommands,
    },
}

#[derive(Subcommand)]
pub enum LimitCommands {
    /// Print the current read or write limit for an agent.
    Get {
        #[command(flatten)]
        agent: AgentRef,
        #[command(flatten)]
        direction: Direction,
    },
    /// Set the read or write limit for an agent.
    Set {
        #[command(flatten)]
        agent: AgentRef,
        #[command(flatten)]
        direction: Direction,
        /// New limit — max summed tool-call cost per interval.
        value: u64,
    },
}

#[derive(Subcommand)]
pub enum IntervalCommands {
    /// Print the current read or write interval for an agent.
    Get {
        #[command(flatten)]
        agent: AgentRef,
        #[command(flatten)]
        direction: Direction,
    },
    /// Set the read or write interval for an agent.
    Set {
        #[command(flatten)]
        agent: AgentRef,
        #[command(flatten)]
        direction: Direction,
        /// Humantime interval, e.g. "1h", "30m", "90s".
        interval: String,
    },
}

#[derive(Subcommand)]
pub enum ToolCommands {
    /// Print the current quota cost of a tool for an agent.
    Get {
        #[command(flatten)]
        agent: AgentRef,
        /// Which MCP tool.
        #[arg(value_enum)]
        tool: ToolName,
    },
    /// Set the quota cost of a tool for an agent.
    Set {
        #[command(flatten)]
        agent: AgentRef,
        /// Which MCP tool.
        #[arg(value_enum)]
        tool: ToolName,
        /// New per-call cost.
        cost: u64,
    },
}

/// `--read` XOR `--write` selector for the quota direction. Exactly one
/// is required (clap `ArgGroup`).
#[derive(Debug, Args)]
#[command(group = ArgGroup::new("direction")
    .required(true)
    .multiple(false)
    .args(["read", "write"]))]
pub struct Direction {
    /// Target the read budget.
    #[arg(long, group = "direction")]
    pub read: bool,
    /// Target the write budget.
    #[arg(long, group = "direction")]
    pub write: bool,
}

impl Direction {
    /// The selected direction. The clap group guarantees exactly one of
    /// `--read`/`--write` is set, so a false `read` means `write`.
    fn resolve(&self) -> QuotaDirection {
        if self.read {
            QuotaDirection::Read
        } else {
            QuotaDirection::Write
        }
    }
}

/// Wire label for a direction, used in `get` notifications.
fn dir_label(dir: QuotaDirection) -> &'static str {
    match dir {
        QuotaDirection::Read => "read",
        QuotaDirection::Write => "write",
    }
}

impl Commands {
    pub async fn handle(self, ctx: &crate::context::Context) -> bool {
        crate::output::emit_result(self.run(ctx).await)
    }

    async fn run(self, ctx: &crate::context::Context) -> Result<Output, Error> {
        match self {
            Commands::Limit { command } => command.run(ctx).await,
            Commands::Interval { command } => command.run(ctx).await,
            Commands::Tool { command } => command.run(ctx).await,
        }
    }
}

impl LimitCommands {
    async fn run(self, ctx: &crate::context::Context) -> Result<Output, Error> {
        match self {
            LimitCommands::Get { agent, direction } => {
                let account = agent.resolve_raw(&ctx.config);
                let dir = direction.resolve();
                let cfg = ctx
                    .db
                    .quota_config(&account)
                    .await
                    .map_err(|e| Error::Other(format!("quota config: {e}")))?;
                let (limit, _) = cfg.for_direction(dir);
                crate::output::OutputResult::Notification(serde_json::json!({
                    "account": account,
                    "direction": dir_label(dir),
                    "limit": limit,
                }))
                .emit();
                Ok(Output::Ok)
            }
            LimitCommands::Set { agent, direction, value } => {
                let account = agent.resolve_raw(&ctx.config);
                let dir = direction.resolve();
                ctx.db
                    .quota_set_limit(&account, dir, value)
                    .await
                    .map_err(|e| Error::Other(format!("set quota limit: {e}")))?;
                Ok(Output::Ok)
            }
        }
    }
}

impl IntervalCommands {
    async fn run(self, ctx: &crate::context::Context) -> Result<Output, Error> {
        match self {
            IntervalCommands::Get { agent, direction } => {
                let account = agent.resolve_raw(&ctx.config);
                let dir = direction.resolve();
                let cfg = ctx
                    .db
                    .quota_config(&account)
                    .await
                    .map_err(|e| Error::Other(format!("quota config: {e}")))?;
                let (_, interval_secs) = cfg.for_direction(dir);
                crate::output::OutputResult::Notification(serde_json::json!({
                    "account": account,
                    "direction": dir_label(dir),
                    "interval_secs": interval_secs,
                    "interval": humantime::format_duration(Duration::from_secs(interval_secs))
                        .to_string(),
                }))
                .emit();
                Ok(Output::Ok)
            }
            IntervalCommands::Set { agent, direction, interval } => {
                let account = agent.resolve_raw(&ctx.config);
                let dir = direction.resolve();
                let secs = humantime::parse_duration(&interval)
                    .map_err(|e| Error::Other(format!("parse interval '{interval}': {e}")))?
                    .as_secs();
                ctx.db
                    .quota_set_interval(&account, dir, secs)
                    .await
                    .map_err(|e| Error::Other(format!("set quota interval: {e}")))?;
                Ok(Output::Ok)
            }
        }
    }
}

impl ToolCommands {
    async fn run(self, ctx: &crate::context::Context) -> Result<Output, Error> {
        match self {
            ToolCommands::Get { agent, tool } => {
                let account = agent.resolve_raw(&ctx.config);
                let cost = ctx
                    .db
                    .quota_tool_cost(&account, tool.as_name())
                    .await
                    .map_err(|e| Error::Other(format!("quota tool cost: {e}")))?;
                crate::output::OutputResult::Notification(serde_json::json!({
                    "account": account,
                    "tool": tool.as_name(),
                    "cost": cost,
                }))
                .emit();
                Ok(Output::Ok)
            }
            ToolCommands::Set { agent, tool, cost } => {
                let account = agent.resolve_raw(&ctx.config);
                ctx.db
                    .quota_set_tool_cost(&account, tool.as_name(), cost)
                    .await
                    .map_err(|e| Error::Other(format!("set quota tool cost: {e}")))?;
                Ok(Output::Ok)
            }
        }
    }
}
