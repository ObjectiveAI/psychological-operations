//! MCP per-tool-call quota — ledger storage only.
//!
//! Metering is keyed by the `account` (agent name) a tool acts as. This
//! crate keeps only a bare append-only ledger of `(account, tool, at)`
//! invocations, plus the default constants the MCP uses as session-arg
//! fallbacks. The per-session limits / interval / per-tool costs now
//! arrive on the MCP session (the `quota_*` arguments), and read/write
//! classification + cost live in the MCP, which reads these rows and
//! applies everything at enforcement time.

use crate::{Db, Error, unix_now};

/// Default trailing read budget (tool calls' summed read cost / interval).
pub const DEFAULT_READ_LIMIT: u64 = 30;
/// Default trailing write budget.
pub const DEFAULT_WRITE_LIMIT: u64 = 10;
/// Default sliding-window length for both directions (1 hour).
pub const DEFAULT_INTERVAL_SECS: u64 = 3600;
/// Default per-tool quota cost when no override is set.
pub const DEFAULT_TOOL_COST: u64 = 1;

impl Db {
    /// Append a tool invocation to the ledger.
    pub async fn record_tool_invocation(&self, account: &str, tool: &str) -> Result<(), Error> {
        sqlx::query("INSERT INTO tool_invocations (account, tool, at) VALUES ($1, $2, $3)")
            .bind(account)
            .bind(tool)
            .bind(unix_now())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// `(tool, count)` for every tool this account invoked at/after
    /// `cutoff` (unix seconds). The MCP filters by direction and
    /// multiplies each count by the tool's cost.
    pub async fn tool_invocation_counts_since(
        &self,
        account: &str,
        cutoff: i64,
    ) -> Result<Vec<(String, u64)>, Error> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT tool, COUNT(*) AS n FROM tool_invocations \
             WHERE account = $1 AND at >= $2 GROUP BY tool",
        )
        .bind(account)
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(t, n)| (t, n as u64)).collect())
    }
}
