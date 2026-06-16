# psychological-operations

> **Note:** This repo is being transformed into an `objectiveai` plugin rather than a standalone binary. It does not work at the moment.

Teach X's [For You](https://x.com) algorithm to surface tweets that match a function you define. Each psyop is an X account paired with a multi-stage scoring pipeline; the run loop captures, scores, and likes/retweets on-target tweets, training the algorithm to surface more of the same. Scoring is powered by [ObjectiveAI](https://github.com/ObjectiveAI/objectiveai).

## Install

psychological-operations shells out to the [`objectiveai`](https://github.com/ObjectiveAI/objectiveai) CLI for all scoring API calls, so it must be installed and on `PATH`. Install both:

### 1. ObjectiveAI CLI (prerequisite)

Install the pre-built binary with one command:

```bash
curl -fsSL https://raw.githubusercontent.com/ObjectiveAI/objectiveai/main/install.sh | bash
. "$HOME/.objectiveai/env"
```

Leaner, no-viewer build:

```bash
curl -fsSL https://raw.githubusercontent.com/ObjectiveAI/objectiveai/main/install.sh | bash -s -- --no-viewer
. "$HOME/.objectiveai/env"
```

Sourcing `~/.objectiveai/env` puts `objectiveai` on `PATH` for the current shell. New shells pick it up automatically (the installer wires `~/.bashrc` / `~/.zshrc` to source the same file).

Supported platforms: Linux x86_64, Linux aarch64 (Raspberry Pi 4+, Graviton), macOS x86_64, macOS aarch64 (Apple Silicon), Windows x86_64. The installer drops the binary at `~/.objectiveai/objectiveai`; the CLI self-updates on startup from [GitHub Releases](https://github.com/ObjectiveAI/objectiveai/releases).

### 2. psychological-operations CLI

Install the pre-built binary with one command:

```bash
curl -fsSL https://raw.githubusercontent.com/WiggidyW/psychological-operations/main/install.sh | bash
. "$HOME/.psychological-operations/env"
```

Sourcing `~/.psychological-operations/env` puts `psychological-operations` on `PATH` for the current shell. New shells pick it up automatically (the installer wires `~/.bashrc` / `~/.zshrc` to source the same file).

Supported platforms: Linux x86_64, macOS x86_64, macOS aarch64 (Apple Silicon), Windows x86_64. The installer drops the binary at `~/.psychological-operations/psychological-operations`, fetched from [GitHub Releases](https://github.com/WiggidyW/psychological-operations/releases).

#### From source

```bash
git clone https://github.com/WiggidyW/psychological-operations.git
cd psychological-operations
bash psychological-operations-cli/install.sh
```

## How it works

A **psyop** is an X account paired with a function that scores any tweet against the question "is this what I want in my feed?". Run the psyop on a schedule and X's recommendation algorithm converges that account's For You feed toward the function's high-scoring distribution.

The loop has four steps:

1. **Capture** — A Chromium extension running on x.com sends every tweet ID it sees in the For You feed to a local queue (`for_you_queue`). The extension only ever sees IDs; nothing leaves the machine until the next run.
2. **Hydrate & filter** — `psyops run` drains the queue, fetches tweet metadata via the X v2 API, and applies per-source filters: thresholds on likes/retweets/replies/impressions, age windows, engagement ratios, or arbitrary [Starlark](https://github.com/bazelbuild/starlark) expressions.
3. **Score** — Surviving tweets pass through an N-stage [ObjectiveAI](https://github.com/ObjectiveAI/objectiveai) pipeline. Each stage is a function + profile + threshold; tweets falling below `output_threshold` or outside `output_top` are dropped before the next stage. One stage works fine for cheap functions; multiple stages let you cascade (e.g. cheap heuristic → mid-tier model → expensive swarm vote) so only promising candidates pay the full cost.
4. **Engage** — The top survivors are liked or retweeted as the psyop's X account via the X API. Authentic engagement is the only signal X's algorithm trusts; consistent on-target likes/retweets train For You to surface more of the same.

The next run captures a more on-target For You feed than the last. After a few cycles, the feed converges. The function defines the destination; the loop drives the algorithm there.

## Anatomy of a psyop

Each psyop is a `psyop.json` published into a git repo (the commit SHA is part of the psyop's identity, so different versions of the same psyop coexist). Fields:

**Sources** — where candidate tweets come from.

- `for_you` — the algorithmic timeline. Captured passively by the Chromium extension as the psyop's account browses x.com. Optional `priority` and `filter`.
- `queries` — a list of X v2 search-operator strings (e.g. `from:user has:media -is:retweet`). Each query has its own optional `priority`, `filter`, and `endpoint` (`recent` for the 7-day window on every X access tier; `all` for the full archive on Pro/Enterprise).

Tweets that show up in both sources are deduped; the priority across accepting sources wins.

**Filters** — per-source eligibility gates.

- Numeric: `min_likes` / `max_likes` / `min_retweets` / `max_retweets` / `min_replies` / `max_replies` / `min_impressions` / `max_impressions` / `min_age` / `max_age`.
- Engagement ratios: `min_likes_per_impression` and friends. Skipped when impressions are zero.
- `custom`: a Starlark boolean expression with `likes`, `retweets`, `replies`, `impressions`, and `age` in scope — `custom: "likes > 100 and retweets / likes > 0.5"`. AND-combined with everything above.

**Sort & caps** — `sort` (`newest` / `oldest` / `likes` / `retweets` / `replies` / Starlark `custom`) tiebreaks within priority buckets. `min_posts` is the floor that triggers query backfill if For You alone didn't deliver enough material. `max_posts` is the hard cap before scoring.

**Stages** — a non-empty list of scoring stages. Each:

- `function`, `profile`, `strategy` — ObjectiveAI scoring config (function defines the question; profile defines the learned weights; strategy is `default` or `swiss-system`).
- `invert` — flip the score (useful when the function is more naturally framed as "how off-target is this?").
- `images` / `videos` — toggle whether media goes into the scoring input.
- `output_threshold` — drop tweets scoring below `[0.0, 1.0]`.
- `output_top` — keep only the top fraction of survivors (e.g. `0.25` = top quarter).

Stage k's survivors become stage k+1's input.

**Cost knob** — `query_when_for_you_queued` (default `true`). Set to `false` to skip search-API queries while the For You queue still has unhydrated material; useful in steady state to avoid burning search credits when the feedback loop is already self-sustaining.

A minimal example:

```json
{
  "for_you": { "filter": { "min_impressions": 1000 } },
  "queries": [
    { "query": "from:vitalikbuterin -is:retweet", "endpoint": "recent", "priority": 10 }
  ],
  "min_posts": 5,
  "max_posts": 100,
  "sort": "newest",
  "stages": [
    {
      "function": { "owner": "you", "repository": "tweet-scorer" },
      "profile":  { "owner": "you", "repository": "tweet-scorer-profile" },
      "strategy": { "type": "default" },
      "output_top": 0.2
    }
  ]
}
```

## Multi-account model & shared billing

Each psyop is a separate X account.

- **One X developer app, shared billing.** All psyops authenticate against a single X Project + App. Credentials live in `~/.psychological-operations/x_app.json` and are captured once via the Chromium extension's setup flow. Every API call — search, hydrate, like, retweet, regardless of which psyop initiated it — deducts from this one credit pool.
- **One OAuth user-context token per psyop.** `psychological-operations psyops browse <name>` opens an embedded Chromium with a profile dedicated to that psyop. Sign into X with whichever account the psyop should act as. `psychological-operations psyops login <name>` then runs a PKCE handshake against a localhost callback and writes per-psyop tokens to `~/.psychological-operations/tokens/<name>.json`. From that point on, the psyop's likes and retweets come from that X account.
- **Why the split?** It lets one developer account fund actions across many real X accounts (alts, brand handles, persona accounts), each with its own follower graph and its own For You algorithm to train. The feedback loop runs independently per psyop; the bill consolidates.

## Delivery

A psyop's `agent_tags` list names the agents that should receive its survivors. After a run scores its tweets, each surviving tweet is written into every listed agent's queue and the agent is notified of its new pending count. The agent works its queue through the `x-api` MCP server, marking tweets handled as it acts on them. A psyop with no `agent_tags` is score-only — it ranks tweets without delivering them anywhere.

Tweets can also be placed on an agent's queue by hand with `agents enqueue --agent-tag <tag> --tweet-id <id> --message <msg>`, which notifies the agent the same way.

## Profile improvement

Functions are *invented* — generated by an ObjectiveAI agent from a description of what you want to find. Profiles are *trained* — given a labeled dataset of tweets the function scored, ObjectiveAI's profile-computation endpoint refines the swarm weights so future runs converge on the labeled ground truth. As you tag results good or bad over time, the profile sharpens and the feedback loop tightens.

## Architecture

- **`psychological-operations-cli`** — Rust CLI. Owns the run loop, persistent state (`posts`, `contents`, `sources`, `scores`, `for_you_queue`, agent `queue`), the X v2 client (auto-generated from the OpenAPI spec), per-psyop OAuth, and shells out to the `objectiveai` CLI for scoring.
- **`psychological-operations-chromium-extension-read`** — MV3 Chromium extension loaded by `psyops browse`. Walks the For You DOM on `x.com` and pipes tweet IDs to the CLI over Chromium native-messaging.
- **`psychological-operations-chromium-extension-auth`** — MV3 Chromium extension loaded by `x_app setup`. 5-field credential form on `developer.x.com` / `console.x.com` that pipes the master X-App credentials to the CLI for `~/.psychological-operations/x_app.json`.
- **`psychological-operations-chromium`** — Bundles a pinned upstream Chromium snapshot + packed/signed CRX3s of both extensions; embedded into the CLI binary at compile time and extracted into per-psyop profile directories at runtime.
- **`crx-pack`** — Build-time tool that packs an unpacked extension into a signed CRX3 with a deterministic key, so the same extension ID is reproduced across builds.

## System requirements

- **Rust** toolchain (stable) — only needed for the from-source install.
- **Windows**: Windows 10 build 17063 or later.
- **macOS / Linux**: any modern version.
- **`objectiveai` CLI** — see [Install](#install).

## Funding

Two billing pools, independent of each other:

- **LLM swarm** — pays for scoring. Every stage is a function execution against a swarm of LLMs. Configure via the `objectiveai` CLI; swarm calls can be funded through OpenRouter credits, an Anthropic API key, or — when running a local ObjectiveAI API server — an Anthropic Pro/Max subscription.
- **X API credits** — pay for hydrate / search / like / retweet calls. Single dev-app pool shared across every psyop.
