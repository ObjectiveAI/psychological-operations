//! Per-caller X-API request log + sliding-window quota ledger (ported
//! from the SDK's `x-api-mcp.sqlite`).
//!
//! Every real X-API HTTP request the MCP's client fires is logged here:
//! the method, the full URL, and the caller's `agent_instance_hierarchy`
//! — never the request arguments. Rows are permanent; this is an audit
//! log first, and the quota mechanism falls out of it: a caller's quota
//! check is "how many same-class (read = GET, write = everything else)
//! rows has this hierarchy logged in the trailing hour" — no buckets, no
//! schedules, no resets.
//!
//! [`Db::request_try_log`] is the whole quota protocol: one guarded
//! `INSERT … SELECT … WHERE count < limit` statement, so the check and
//! the deduction (the inserted row) are atomic under concurrent calls.

use crate::{Db, Error, unix_now};

impl Db {
    /// Atomic check-and-deduct. Classifies `method` as read (GET) or
    /// write (anything else), counts the caller's same-class rows in the
    /// trailing hour, and inserts the new log row iff that count is
    /// below `limit` — one guarded statement, so concurrent callers
    /// can't both squeeze through the last slot.
    ///
    /// `Ok(true)` — logged; the request may fire.
    /// `Ok(false)` — quota hit; nothing was logged.
    pub async fn request_try_log(
        &self,
        agent_instance_hierarchy: &str,
        method: &str,
        url: &str,
        limit: u64,
    ) -> Result<bool, Error> {
        let now = unix_now();
        let cutoff = now - 3600;
        let inserted = sqlx::query(
            "INSERT INTO api_requests \
                 (agent_instance_hierarchy, method, url, requested_at) \
             SELECT $1, $2, $3, $4 \
             WHERE (SELECT COUNT(*) FROM api_requests \
                    WHERE agent_instance_hierarchy = $1 \
                      AND requested_at > $5 \
                      AND (method = 'GET') = ($2 = 'GET')) < $6",
        )
        .bind(agent_instance_hierarchy)
        .bind(method)
        .bind(url)
        .bind(now)
        .bind(cutoff)
        .bind(limit as i64)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(inserted > 0)
    }

    /// `(reads, writes)` this caller has logged in the trailing hour —
    /// the same counts [`Self::request_try_log`] gates on. Read AFTER a
    /// tool's API calls, it reflects that call's own deductions.
    pub async fn request_usage(
        &self,
        agent_instance_hierarchy: &str,
    ) -> Result<(u64, u64), Error> {
        let cutoff = unix_now() - 3600;
        let row: (i64, i64) = sqlx::query_as(
            "SELECT \
                 COUNT(*) FILTER (WHERE method = 'GET'), \
                 COUNT(*) FILTER (WHERE method <> 'GET') \
             FROM api_requests \
             WHERE agent_instance_hierarchy = $1 AND requested_at > $2",
        )
        .bind(agent_instance_hierarchy)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await?;
        Ok((row.0.max(0) as u64, row.1.max(0) as u64))
    }
}
