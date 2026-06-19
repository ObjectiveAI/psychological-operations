//! MCP per-tool-call quota — ledger storage only.
//!
//! Metering is keyed by the `account` (agent name) a tool acts as. This
//! crate keeps only a bare append-only ledger of `(account, tool, at)`
//! invocations, plus the default constants the MCP uses as session-arg
//! fallbacks. The per-session limits / interval / per-tool costs now
//! arrive on the MCP session (the `quota_*` arguments), and read/write
//! classification + cost live in the MCP, which reads these rows and
//! applies everything at enforcement time.

use crate::{unix_now, Db, Error};

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

    /// Record a time-bounded additive quota grant for `account` in one
    /// `direction` (`"read"` / `"write"`). `amount` is added to that
    /// direction's available quota while `granted_at <= now < expires_at`.
    pub async fn grant_quota(
        &self,
        account: &str,
        direction: &str,
        amount: i64,
        granted_at: i64,
        expires_at: i64,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO quota_grants \
             (account, direction, amount, granted_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(account)
        .bind(direction)
        .bind(amount)
        .bind(granted_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Total of all grants for `account` + `direction` in effect at `now`
    /// (unix seconds). Active grants stack; `0` when none.
    pub async fn active_quota_grants(
        &self,
        account: &str,
        direction: &str,
        now: i64,
    ) -> Result<i64, Error> {
        // `SUM(bigint)` yields NUMERIC in postgres; cast back to BIGINT so
        // `query_scalar` can decode it as `i64` (otherwise: "mismatched
        // types; Rust type i64 (INT8) is not compatible with SQL type
        // NUMERIC" on every quota-checked tool call once any grant exists).
        let total: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount), 0)::bigint FROM quota_grants \
             WHERE account = $1 AND direction = $2 \
               AND granted_at <= $3 AND expires_at > $3",
        )
        .bind(account)
        .bind(direction)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(total)
    }
}
