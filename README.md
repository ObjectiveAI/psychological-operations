# psychological-operations

An [ObjectiveAI](https://github.com/ObjectiveAI/objectiveai) **plugin** that runs
autonomous persona **agents** on **X (Twitter)** and **Discord**. Each agent is
an X account plus a Discord bot, addressed by a `tag`. The plugin gives every
agent:

- **Tool-mediated presence** on both platforms — two MCP servers (`x` and
  `discord`) the agent uses to read and act (post, reply, like, DM, react, …).
- **Psyops** — scheduled pipelines that ingest posts/messages from X and/or
  Discord, score them through an ObjectiveAI LLM swarm, and deliver the survivors
  to agents' work queues.
- **Event-driven wake-ups** — a resident daemon that listens to Discord's
  gateway and wakes an agent when it's mentioned, replied to, or DM'd.

ObjectiveAI supplies the agent runtime and the scoring; this plugin supplies the
platforms, the pipelines, and the persistence.

---

## Core concepts

- **Agent / persona** — a `tag` (e.g. `dr-strange`) bound to an X account (via
  OAuth) and/or a Discord bot. Every tool call, queued item, and hook is scoped
  to a tag; the agent acts *as* that identity.
- **MCP servers** (`x`, `discord`) — the agent's hands on each platform. Each
  session carries a `mode` (`readonly` / `full`), a per-tag rate **quota**, and a
  postgres-backed **response cache**. The Discord server also honors a
  `max_message_length` (default 2000).
- **Psyop** — an ingest → filter → score → deliver pipeline. A psyop is either an
  X psyop or a Discord psyop; it runs `manual`ly or on an `interval`.
- **Daemon** — one resident process that (a) holds a Discord gateway connection
  per eligible agent and fires that agent's **hooks**, and (b) schedules due
  psyop runs.
- **Hooks** — per-agent rules that wake an agent on a Discord event.

---

## Agent tools (MCP)

Agents reach the platforms exclusively through these tools. Read/write split
maps to the session `mode`; write tools are hidden in `readonly`.

### `x` server

| group | tools |
|---|---|
| read | `run_query`, `list_timeline`, `list_mentions`, `list_replies`, `list_bookmarks`, `list_following`, `list_followers`, `get_tweet`, `get_bio`, `get_profile_picture`, `open_attachment`, `whoami` |
| write | `post`, `reply`, `quote`, `like`, `retweet`, `bookmark`, `follow`, `unfollow` |
| queue | `read_queue`, `mark_handled` |

### `discord` server

| group | tools |
|---|---|
| read | `list_servers`, `list_channels`, `list_users`, `list_role_members`, `get_role`, `get_user`, `get_profile_picture`, `list_messages`, `get_message`, `list_available_reactions`, `get_message_reactions_by_user`, `open_attachment`, `whoami` |
| write | `send_message`, `send_direct_message`, `edit_message`, `delete_message`, `create_thread`, `add_reaction`, `remove_reaction` |
| queue | `read_queue`, `mark_handled` |
| other | `invite_link` |

Reads are served from a shared response cache (per-user for identity/visibility
calls like `whoami` / `list_servers` / `list_channels`; global for content like
messages, users, and roles). Writes always go straight through. Outgoing Discord
content is length-checked up front against `max_message_length` so the agent gets
a clear "shorten it" error instead of a Discord rejection.

---

## Psyops

A psyop ingests candidates, filters and ranks them, optionally scores them
through ObjectiveAI, and delivers the survivors to one or more agents' queues —
which the agents then work through their MCP `read_queue` / `mark_handled` tools.

**Sources** (at least one required; each names the `agent_tag` whose auth it
reads as, plus an optional `priority` and per-source filter):

- **X** — `queries` (X v2 recent search), `timeline` (an agent's reverse-chron
  home timeline), `mentions` (an agent's mentions), `for_you` (the algorithmic
  feed). String/numeric `filter` (engagement thresholds, age windows, ratios) or
  a Python boolean expression.
- **Discord** — `channels` (a channel's recent messages) and `servers` (across a
  guild's text channels). Each takes a `count` and an optional `python_filter`.

**Pipeline knobs:**

- `trigger` — `manual` (runs only when named) or `interval` (a humantime cadence
  the daemon scheduler honors).
- `sort` — tiebreak ordering within priority buckets (`newest` / `oldest` /
  engagement / a Python expression).
- `stages` — an optional N-stage ObjectiveAI scoring pipeline. Each stage is a
  function + profile + strategy with `output_threshold` / `output_top` narrowing;
  stage *k*'s survivors feed stage *k+1*. No stages ⇒ every survivor passes at a
  flat score.
- `agent_tags` — deliver survivors to these agents' queues and notify them. Empty
  ⇒ score-only (rank, deliver nothing).
- `message` — a note delivered alongside the queued items so the agent knows what
  the run is for.

**Manage:** `psyops insert` (upsert from inline JSON or a file), `psyops get`,
`psyops list [--x|--discord]`, `psyops enable|disable`, `psyops run [--name …]`,
and `psyops schema` (emits the exact JSON Schema `insert` accepts).

A minimal X psyop (run `psyops schema` for the full shape; Discord psyops are
symmetric, swapping `queries`/etc. for `channels`/`servers`):

```json
{
  "type": "x",
  "queries": [
    { "query": "from:vitalikbuterin -is:retweet", "agent_tag": "riddler", "max_posts": 100, "priority": 10 }
  ],
  "trigger": { "type": "interval", "interval": "1h" },
  "sort": "newest",
  "stages": [
    {
      "function": { "owner": "you", "repository": "tweet-scorer" },
      "profile":  { "owner": "you", "repository": "tweet-scorer-profile" },
      "strategy": { "type": "default" },
      "output_top": 0.2
    }
  ],
  "agent_tags": ["riddler"],
  "message": "On-topic takes worth engaging with."
}
```

Items can also be queued by hand: `agents enqueue x --agent-tag T --tweet-id ID
--message M` or `agents enqueue discord --agent-tag T --channel-id C --message-id
M --message MSG`.

---

## Hooks

Hooks wake an agent on Discord gateway events. The daemon evaluates them per
event for every eligible agent. Four types:

| type | fires when |
|---|---|
| `python` | every gateway event (operator Python runs with the raw event as input) |
| `mention` | a message `@everyone`s, mentions the agent, or mentions a role the agent holds |
| `reply` | someone replies to the agent's message |
| `dm` | the agent receives a direct message |

The declarative types (`mention` / `reply` / `dm`) carry an optional `user_id`
(defaulting to the bot's own id) and a `message`. On a match the daemon enqueues
the triggering message to the agent and notifies it — and a built-in self-filter
means an agent never triggers its own hooks.

**Manage:** `agents daemon discord hooks insert <python|mention|reply|dm> …`,
plus `hooks list`, `hooks get`, `hooks delete`.

---

## Auth & onboarding

Set up once, then per agent:

1. **Master X App** — `x-app setup` captures the X developer App credentials via
   the embedded browser (stored in `x_app_credentials`). Shared by every agent's
   X calls.
2. **Per-agent X login** — `agents login x --agent-tag T` runs the X OAuth 2.0
   PKCE flow in a browser profile dedicated to that agent.
3. **Per-agent Discord bot** — `agents login discord --agent-tag T` walks the
   Discord developer-portal bot-creation wizard and stores the bot token
   (`discord_auth`).
4. **Invite the bot** — `agents invite discord --agent-tag T` prints the bot's
   server-invite URL.

`--dangerously-reset` on the login commands wipes an agent's existing state for a
clean re-login. Per-agent rate budgets can be topped up with `agents quota grant
x|discord --mode read|write --agent-tag T --quantity N --duration D`.

---

## Architecture

A Rust workspace plus a JS viewer, packaged as one ObjectiveAI plugin.

- **`psychological-operations-cli`** — the plugin binary. Owns the command tree
  (`psyops`, `agents`, `x-app`, `mcp`, `daemon`) and the psyop run pipeline.
- **`psychological-operations-x-mcp`** / **`psychological-operations-discord-mcp`**
  — the two MCP servers (tools, per-session quota, mode gating, response cache).
- **`psychological-operations-sdk`** — shared types and the X & Discord clients
  (response caching, the dev-console credential parsers, serenity-backed Discord
  REST + gateway).
- **`psychological-operations-db`** — the single postgres/sqlx layer: per-plugin
  schema, the per-agent work queues, hooks, auth stores, and the response cache.
- **`psychological-operations-browser`** — a CEF-based browser that drives the
  interactive flows (`x-app setup`, `agents login x|discord`, `agents browser`).
- **`psychological-operations-viewer`** — a separate web app for inspecting
  psyops (wired via the manifest's `viewer_routes`).

---

## Build, install & run

This is a plugin, not a standalone tool — it runs inside an ObjectiveAI host,
which provides the agent runtime, a per-state postgres, and the scoring backend.

**Build + install from source** (into `~/.objectiveai/…/<version>/`):

```bash
bash install.sh --from-source           # debug build
bash install.sh --from-source-release   # release build
```

`install.sh` runs `build.sh` (which builds the CLI + bundled CEF browser and the
viewer in parallel and zips them) and installs the result into the objectiveai
plugin tree. `--dir <dir>` overrides the target.

**Running** — the ObjectiveAI host launches everything; you don't invoke the
binary directly:

- one-shot plugin commands → `objectiveai plugins run --owner ObjectiveAI --name
  psychological-operations --version <v> --args '["psyops","list"]'`
- the resident daemon (gateway hooks + psyop scheduler) → `objectiveai daemon
  spawn` (kill with `objectiveai daemon kill`)
- the MCP servers → launched on demand by the host's MCP conduit
- agents → `objectiveai agents spawn --agent-tag T --simple "…"`

The manifest (`objectiveai.json`) declares `daemon: true` and the two MCP
servers (`x`, `discord`).
