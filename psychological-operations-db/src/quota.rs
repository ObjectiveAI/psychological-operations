//! MCP per-account, per-tool-call quota — storage only.
//!
//! Metering moved off X-API HTTP requests and onto MCP tool calls,
//! keyed by the `account` (agent name) a tool acts as. This crate stores
//! three things and computes nothing about read/write or cost — that
//! classification lives in the MCP, which reads these rows and applies
//! each tool's direction + cost at enforcement time:
//!
//!   * [`Db::quota_config`] — per-account read/write limit + interval,
//!     with the module defaults applied for anything unset.
//!   * per-account per-tool cost overrides (default [`DEFAULT_TOOL_COST`]).
//!   * a bare append-only ledger of `(account, tool, at)` invocations.

use std::collections::HashMap;

use crate::{Db, Error, unix_now};

/// Default trailing read budget (tool calls' summed read cost / interval).
pub const DEFAULT_READ_LIMIT: u64 = 30;
/// Default trailing write budget.
pub const DEFAULT_WRITE_LIMIT: u64 = 10;
/// Default sliding-window length for both directions (1 hour).
pub const DEFAULT_INTERVAL_SECS: u64 = 3600;
/// Default per-tool quota cost when no override is set.
pub const DEFAULT_TOOL_COST: u64 = 1;

/// One direction of the quota. The db crate is storage-only, but a
/// direction selector keeps the column-targeting setters tidy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaDirection {
    Read,
    Write,
}

/// Per-account limits + intervals, with defaults already applied.
#[derive(Debug, Clone, Copy)]
pub struct QuotaConfig {
    pub read_limit: u64,
    pub write_limit: u64,
    pub read_interval_secs: u64,
    pub write_interval_secs: u64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            read_limit: DEFAULT_READ_LIMIT,
            write_limit: DEFAULT_WRITE_LIMIT,
            read_interval_secs: DEFAULT_INTERVAL_SECS,
            write_interval_secs: DEFAULT_INTERVAL_SECS,
        }
    }
}

impl QuotaConfig {
    /// `(limit, interval_secs)` for one direction.
    pub fn for_direction(&self, dir: QuotaDirection) -> (u64, u64) {
        match dir {
            QuotaDirection::Read => (self.read_limit, self.read_interval_secs),
            QuotaDirection::Write => (self.write_limit, self.write_interval_secs),
        }
    }
}

impl Db {
    /// Load an account's quota limits/intervals, substituting the module
    /// defaults for a missing row or any unset (NULL) column.
    pub async fn quota_config(&self, account: &str) -> Result<QuotaConfig, Error> {
        let row: Option<(Option<i64>, Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT read_limit, write_limit, read_interval_secs, write_interval_secs \
             FROM quota_config WHERE account = $1",
        )
        .bind(account)
        .fetch_optional(&self.pool)
        .await?;
        let d = QuotaConfig::default();
        Ok(match row {
            None => d,
            Some((rl, wl, ri, wi)) => QuotaConfig {
                read_limit: rl.map(|v| v as u64).unwrap_or(d.read_limit),
                write_limit: wl.map(|v| v as u64).unwrap_or(d.write_limit),
                read_interval_secs: ri.map(|v| v as u64).unwrap_or(d.read_interval_secs),
                write_interval_secs: wi.map(|v| v as u64).unwrap_or(d.write_interval_secs),
            },
        })
    }

    /// Set one direction's limit for an account (upsert).
    pub async fn quota_set_limit(
        &self,
        account: &str,
        dir: QuotaDirection,
        limit: u64,
    ) -> Result<(), Error> {
        let col = match dir {
            QuotaDirection::Read => "read_limit",
            QuotaDirection::Write => "write_limit",
        };
        self.quota_upsert_col(account, col, limit as i64).await
    }

    /// Set one direction's sliding-window interval (seconds) for an
    /// account (upsert).
    pub async fn quota_set_interval(
        &self,
        account: &str,
        dir: QuotaDirection,
        secs: u64,
    ) -> Result<(), Error> {
        let col = match dir {
            QuotaDirection::Read => "read_interval_secs",
            QuotaDirection::Write => "write_interval_secs",
        };
        self.quota_upsert_col(account, col, secs as i64).await
    }

    /// Upsert a single `quota_config` column. `col` is a fixed literal
    /// from the setters above — never user input.
    async fn quota_upsert_col(&self, account: &str, col: &str, value: i64) -> Result<(), Error> {
        let sql = format!(
            "INSERT INTO quota_config (account, {col}) VALUES ($1, $2) \
             ON CONFLICT (account) DO UPDATE SET {col} = excluded.{col}"
        );
        sqlx::query(&sql)
            .bind(account)
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// An account's cost for one tool, or [`DEFAULT_TOOL_COST`] if unset.
    pub async fn quota_tool_cost(&self, account: &str, tool: &str) -> Result<u64, Error> {
        let cost: Option<i64> =
            sqlx::query_scalar("SELECT cost FROM quota_tool_cost WHERE account = $1 AND tool = $2")
                .bind(account)
                .bind(tool)
                .fetch_optional(&self.pool)
                .await?;
        Ok(cost.map(|c| c as u64).unwrap_or(DEFAULT_TOOL_COST))
    }

    /// Set an account's cost for one tool (upsert).
    pub async fn quota_set_tool_cost(
        &self,
        account: &str,
        tool: &str,
        cost: u64,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO quota_tool_cost (account, tool, cost) VALUES ($1, $2, $3) \
             ON CONFLICT (account, tool) DO UPDATE SET cost = excluded.cost",
        )
        .bind(account)
        .bind(tool)
        .bind(cost as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All per-tool cost overrides for an account (tools without an
    /// override are absent; the caller applies [`DEFAULT_TOOL_COST`]).
    pub async fn quota_tool_costs(&self, account: &str) -> Result<HashMap<String, u64>, Error> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT tool, cost FROM quota_tool_cost WHERE account = $1")
                .bind(account)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(t, c)| (t, c as u64)).collect())
    }

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
