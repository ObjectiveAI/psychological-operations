-- psychological-operations: the complete postgres schema.
--
-- Every table our system owns. Run idempotently on Db::connect via
-- sqlx::raw_sql (all CREATE … IF NOT EXISTS). Schema changes = edit
-- this file (greenfield DB; no migration framework, no schema_version
-- table). The Chromium cookie jar is NOT here — it's the browser's own
-- on-disk SQLite, read read-only by cookies.rs.
--
-- Notable departures from the old per-store SQLite schemas:
--   * No `psyop_commit_sha` anywhere — psyops are keyed by name only
--     (git + commit versioning was dropped).
--   * No `locks` table — cross-process mutual exclusion uses postgres
--     advisory locks (locker.rs).
--   * No `schema_version` table — single idempotent schema file.
--   * unix-seconds columns are BIGINT; audit timestamps are
--     TIMESTAMPTZ DEFAULT now(); JSON payloads are JSONB; blobs BYTEA.

-- ── psyop pipeline (ported from cli/data.db, commit_sha removed) ──────

CREATE TABLE IF NOT EXISTS posts (
    -- Monotonic insertion order; postgres has no rowid. for_you-origin
    -- tweets sort by this (browser-arrival order) in bucket_sort.
    seq          BIGSERIAL,
    id           TEXT   NOT NULL,
    psyop        TEXT   NOT NULL,
    handle       TEXT   NOT NULL,
    created      TEXT   NOT NULL,
    likes        BIGINT NOT NULL DEFAULT 0,
    retweets     BIGINT NOT NULL DEFAULT 0,
    replies      BIGINT NOT NULL DEFAULT 0,
    impressions  BIGINT NOT NULL DEFAULT 0,
    ingested_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (id, psyop)
);
CREATE INDEX IF NOT EXISTS posts_by_psyop ON posts(psyop);

CREATE TABLE IF NOT EXISTS sources (
    post_id     TEXT NOT NULL,
    for_you     BOOLEAN NOT NULL,
    query       TEXT,
    sourced_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((for_you AND query IS NULL) OR (NOT for_you AND query IS NOT NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS sources_unique
    ON sources(post_id, COALESCE(query, ''));

CREATE TABLE IF NOT EXISTS contents (
    post_id  TEXT PRIMARY KEY,
    text     TEXT  NOT NULL,
    images   JSONB NOT NULL DEFAULT '[]'::jsonb,
    videos   JSONB NOT NULL DEFAULT '[]'::jsonb
);

CREATE TABLE IF NOT EXISTS scores (
    post_id    TEXT PRIMARY KEY,
    score      DOUBLE PRECISION NOT NULL,
    scored_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS for_you_queue (
    post_id      TEXT NOT NULL,
    psyop        TEXT NOT NULL,
    ingested_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (post_id, psyop)
);
CREATE INDEX IF NOT EXISTS for_you_queue_by_psyop ON for_you_queue(psyop);

CREATE TABLE IF NOT EXISTS psyop_runs (
    psyop        TEXT   PRIMARY KEY,
    last_run_at  BIGINT NOT NULL
);

-- ── X-API response cache (ported from sdk/x-api-cache.sqlite) ─────────

CREATE TABLE IF NOT EXISTS cache (
    key          BYTEA  PRIMARY KEY,
    body         BYTEA  NOT NULL,
    inserted_at  BIGINT NOT NULL
);

-- ── per-agent tweet queue (ported from sdk/queue.sqlite) ──────────────

CREATE TABLE IF NOT EXISTS queue (
    agent_tag                          TEXT   NOT NULL,
    tweet_id                           TEXT   NOT NULL,
    psyop                              TEXT,
    score                              DOUBLE PRECISION,
    deliverer_agent_instance_hierarchy TEXT,
    message                            TEXT,
    queued_at                          BIGINT NOT NULL,
    PRIMARY KEY (agent_tag, tweet_id)
);

-- ── deferred reply/quote queue ───────────────────────────────────────
--
-- When X refuses a reply/quote with the conversation-restriction 403, the
-- x-api MCP captures the attempt here (one pending reply + one pending
-- quote per agent per tweet) instead of failing it. Delivery is separate.

CREATE TABLE IF NOT EXISTS reply_quote_queue (
    agent_tag        TEXT   NOT NULL,
    kind             TEXT   NOT NULL,  -- 'reply' | 'quote'
    target_tweet_id  TEXT   NOT NULL,  -- in_reply_to_tweet_id / quote_tweet_id
    text             TEXT   NOT NULL,  -- the reply/quote body
    queued_at        BIGINT NOT NULL,
    PRIMARY KEY (agent_tag, kind, target_tweet_id)
);

-- ── per-agent per-target action dedup (idempotency) ──────────────────
-- At most one of each action per (account, target). tweet-ID target for
-- like/retweet/quote/reply; normalized handle for follow. quote<->retweet
-- are mutually exclusive (the pre-check scans both). unfollow DELETEs the
-- matching follow row so a later follow is allowed again.

CREATE TABLE IF NOT EXISTS actions (
    account TEXT   NOT NULL,  -- agent tag (same key as the quota ledger)
    action  TEXT   NOT NULL,  -- 'like' | 'retweet' | 'quote' | 'reply' | 'follow'
    target  TEXT   NOT NULL,  -- tweet_id, or normalized handle for follow
    at      BIGINT NOT NULL,  -- unix seconds
    PRIMARY KEY (account, action, target)
);

-- ── MCP per-account, per-tool-call quota ledger ──────────────────────
-- Metering is on MCP TOOL CALLS, not X-API HTTP requests, keyed by the
-- `account` (agent name) a tool acts as. The ledger is intentionally
-- dumb — bare invocations, no cost/direction stored. Limits, interval,
-- and per-tool costs now arrive per-session on the MCP `quota_*`
-- arguments; the MCP applies them (+ each tool's read/write direction)
-- against this ledger at enforcement time.

-- Bare per-account tool-invocation ledger (no cost, no direction).
CREATE TABLE IF NOT EXISTS tool_invocations (
    id      BIGSERIAL PRIMARY KEY,
    account TEXT   NOT NULL,
    tool    TEXT   NOT NULL,
    at      BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS tool_invocations_account_time
    ON tool_invocations(account, at);

-- ── time-bounded additive quota grants ───────────────────────────────
-- A grant gives one account a flat boost to its read/write available
-- quota while in effect (`granted_at <= now < expires_at`). Append-only,
-- so multiple active grants stack (sum). The MCP adds the active total to
-- the per-direction limit at enforcement time.

CREATE TABLE IF NOT EXISTS quota_grants (
    id          BIGSERIAL PRIMARY KEY,
    account     TEXT   NOT NULL,  -- the agent tag (the quota-ledger key)
    direction   TEXT   NOT NULL,  -- 'read' | 'write'
    amount      BIGINT NOT NULL,  -- flat boost added to the limit while in effect
    granted_at  BIGINT NOT NULL,  -- unix seconds
    expires_at  BIGINT NOT NULL   -- granted_at + duration, unix seconds
);
CREATE INDEX IF NOT EXISTS quota_grants_account_dir_exp
    ON quota_grants(account, direction, expires_at);

-- ── psyops (was git repos + psyop.json) ──────────────────────────────

CREATE TABLE IF NOT EXISTS psyops (
    name        TEXT  PRIMARY KEY,
    definition  JSONB NOT NULL,
    disabled    BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── X-App scraped credential HTML (was x_app.json + html) ────────────

CREATE TABLE IF NOT EXISTS x_app_html (
    handle    TEXT NOT NULL,  -- normalized X handle / numeric twid
    kind      TEXT NOT NULL,  -- 'post_create_dialog' | 'oauth_popup'
    html      TEXT NOT NULL,
    saved_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (handle, kind)
);

-- ── persona identity: persona (agent/psyop) → the X account it operates ──
--
-- Established by the login browser once it observes the signed-in `twid`
-- cookie. Every runtime auth decision reads this instead of the cookie, so
-- nothing outside the browser touches the CEF cookie store.

CREATE TABLE IF NOT EXISTS persona_twids (
    kind          TEXT NOT NULL,  -- 'psyop' | 'agent'
    name          TEXT NOT NULL,
    persona_twid  TEXT NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, name)
);

-- ── account auth: X account twid → OAuth token ──────────────────────────
--
-- Keyed by `persona_twid` alone — an X-App reset wipes the whole table, so
-- only one X-App's tokens ever exist at a time and the twid is unique.
-- `x_app_twid` rides along for token refresh + provenance.

CREATE TABLE IF NOT EXISTS account_auth (
    persona_twid  TEXT PRIMARY KEY,
    x_app_twid    TEXT NOT NULL,
    tokens        JSONB NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
