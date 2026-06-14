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

CREATE TABLE IF NOT EXISTS delivery_queue (
    id               BIGSERIAL PRIMARY KEY,
    psyop            TEXT  NOT NULL,
    target           JSONB NOT NULL,
    post_ids         JSONB NOT NULL,
    attempts         BIGINT NOT NULL DEFAULT 0,
    last_error       TEXT,
    last_attempt_at  TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS delivery_queue_by_psyop ON delivery_queue(psyop);

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
    agent                              TEXT   NOT NULL,
    agent_kind                         TEXT   NOT NULL,
    tweet_id                           TEXT   NOT NULL,
    psyop                              TEXT,
    score                              DOUBLE PRECISION,
    deliverer_agent_instance_hierarchy TEXT,
    message                            TEXT,
    queued_at                          BIGINT NOT NULL,
    PRIMARY KEY (agent, tweet_id)
);

-- ── MCP engagement records (ported from sdk/x-api-mcp.sqlite) ─────────

CREATE TABLE IF NOT EXISTS replies (
    tweet_id TEXT NOT NULL, agent TEXT NOT NULL, created_at BIGINT NOT NULL,
    PRIMARY KEY (tweet_id, agent)
);
CREATE TABLE IF NOT EXISTS retweets (
    tweet_id TEXT NOT NULL, agent TEXT NOT NULL, created_at BIGINT NOT NULL,
    PRIMARY KEY (tweet_id, agent)
);
CREATE TABLE IF NOT EXISTS likes (
    tweet_id TEXT NOT NULL, agent TEXT NOT NULL, created_at BIGINT NOT NULL,
    PRIMARY KEY (tweet_id, agent)
);
CREATE TABLE IF NOT EXISTS quotes (
    tweet_id TEXT NOT NULL, agent TEXT NOT NULL, created_at BIGINT NOT NULL,
    PRIMARY KEY (tweet_id, agent)
);
CREATE TABLE IF NOT EXISTS follows (
    user_id TEXT NOT NULL, agent TEXT NOT NULL, created_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, agent)
);

-- ── MCP per-account, per-tool-call quota ─────────────────────────────
-- Metering is on MCP TOOL CALLS, not X-API HTTP requests, keyed by the
-- `account` (agent name) a tool acts as. The ledger is intentionally
-- dumb — bare invocations, no cost/direction stored; the MCP applies
-- each tool's read/write classification + per-tool cost at query time.

-- Per-account limits + sliding-window intervals. Missing row/columns →
-- the db crate's code defaults (read 30, write 10, interval 1h).
CREATE TABLE IF NOT EXISTS quota_config (
    account             TEXT PRIMARY KEY,
    read_limit          BIGINT,
    write_limit         BIGINT,
    read_interval_secs  BIGINT,
    write_interval_secs BIGINT
);

-- Per-account per-tool cost override (default 1 when absent).
CREATE TABLE IF NOT EXISTS quota_tool_cost (
    account TEXT   NOT NULL,
    tool    TEXT   NOT NULL,
    cost    BIGINT NOT NULL,
    PRIMARY KEY (account, tool)
);

-- Bare per-account tool-invocation ledger (no cost, no direction).
CREATE TABLE IF NOT EXISTS tool_invocations (
    id      BIGSERIAL PRIMARY KEY,
    account TEXT   NOT NULL,
    tool    TEXT   NOT NULL,
    at      BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS tool_invocations_account_time
    ON tool_invocations(account, at);

-- ── psyops (was git repos + psyop.json) ──────────────────────────────

CREATE TABLE IF NOT EXISTS psyops (
    name        TEXT  PRIMARY KEY,
    definition  JSONB NOT NULL,
    disabled    BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── delivery targets (was config.json) ───────────────────────────────
-- Ordered lists; `ord` is a gapless 0-based index matching the
-- `targets list/add/del <index>` surface.

-- One-time guard so the first run can seed the default global targets
-- (X-Like + Stdout) exactly once. After seeding, an operator who does
-- `targets del` down to an empty list keeps it empty — the absence of
-- rows is no longer re-interpreted as "needs defaults". Replaces the
-- old "config.json absent vs present-but-empty" distinction.
CREATE TABLE IF NOT EXISTS config_state (
    singleton              BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    global_targets_seeded  BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE IF NOT EXISTS global_targets (
    ord     INTEGER PRIMARY KEY,
    target  JSONB   NOT NULL
);
CREATE TABLE IF NOT EXISTS psyop_targets (
    psyop   TEXT    NOT NULL,
    ord     INTEGER NOT NULL,
    target  JSONB   NOT NULL,
    PRIMARY KEY (psyop, ord)
);

-- ── X-App master credentials + scraped HTML (was x_app.json + html) ──

CREATE TABLE IF NOT EXISTS x_app (
    singleton      BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    client_id      TEXT,
    client_secret  TEXT,
    bearer_token   TEXT,
    saved_at       TEXT
);

CREATE TABLE IF NOT EXISTS x_app_html (
    handle    TEXT NOT NULL,  -- normalized X handle / numeric twid
    kind      TEXT NOT NULL,  -- 'post_create_dialog' | 'oauth_popup'
    html      TEXT NOT NULL,
    saved_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (handle, kind)
);

-- ── per-persona OAuth tokens (was browser/.../auth.json) ─────────────

CREATE TABLE IF NOT EXISTS auth_tokens (
    kind          TEXT NOT NULL,  -- 'psyop' | 'agent'
    name          TEXT NOT NULL,
    persona_twid  TEXT NOT NULL,
    x_app_twid    TEXT NOT NULL,
    tokens        JSONB NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, name, persona_twid, x_app_twid)
);
