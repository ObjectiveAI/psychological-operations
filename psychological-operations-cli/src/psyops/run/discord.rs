//! The Discord psyop run pipeline.
//!
//! Mirrors the X pipeline ([`super::x`]) but for Discord messages, which are
//! addressed by `(channel_id, message_id)` and carry no engagement metrics.
//! Each psyop runs independently and concurrently:
//!
//! 1. **Ingest** — for every `channel` / `server` source, page the message
//!    history as that source's bot (serenity REST), turning each message into
//!    a [`Post`] (`id` = message id) paired with its channel id.
//! 2. **Filter** — a source's optional `python_filter` runs per message
//!    (a Discord-message dict is the Python `input`); only `True` survives.
//! 3. **Sort** — bucket by source `priority` (smaller first, `None` last),
//!    ordering within each bucket by the psyop's `SortBy` (newest / oldest by
//!    timestamp, or a Python expression).
//! 4. **Dedup** — keep the first occurrence of each message id.
//! 5. **Already-delivered** — drop anything this psyop already delivered
//!    ([`Db::discord_already_delivered`](psychological_operations_db::Db)).
//! 6. **Score** — the multi-stage pipeline (bare = flat 1.0; function = an
//!    objectiveai execution over the per-message input), narrowing by
//!    `output_threshold` / `output_top` each stage.
//! 7. **Deliver** — enqueue survivors to each `agent_tags` entry, mark them
//!    delivered, and notify the agents.
//!
//! Unlike X, there is no `stage_retry` ledger for Discord: a psyop whose
//! scoring stages fail simply re-ingests on its next run.

use std::collections::{BTreeMap, HashMap, HashSet};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use objectiveai_sdk::functions::executions::request::Strategy;
use objectiveai_sdk::functions::{
    FullInlineFunctionOrRemoteCommitOptional, InlineProfileOrRemoteCommitOptional,
};

use psychological_operations_sdk::cli::psyops::discord::{OutputTop, PsyOp, SortBy, Stage};
use psychological_operations_sdk::discord::{self, serenity, GetMessages};
use serenity::all::{ChannelId, ChannelType, GuildId, Message, MessageId};

use crate::db::{MediaUrl, Post};
use crate::error::Error;
use crate::score::{self, ScoredPost};

/// Run the Discord psyops, each concurrently. Trigger gating and the per-psyop
/// lock are handled by the caller ([`super::run_all_inner`]); this resolves
/// agent auth, ingests, scores, and delivers. A per-psyop failure is emitted
/// (`PsyopRunFailed`) and the batch keeps running.
pub(super) async fn run_batch(
    psyops: Vec<(String, PsyOp)>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) {
    if psyops.is_empty() {
        return;
    }
    let client = discord::Client::new(ctx.db.clone(), ctx.cache_max_size, ctx.cache_ttl);
    let mut inflight: FuturesUnordered<_> = psyops
        .iter()
        .map(|(name, psyop)| {
            let client = &client;
            async move {
                (
                    name.as_str(),
                    run_psyop(psyop, name, client, seed, ctx).await,
                )
            }
        })
        .collect();
    while let Some((name, result)) = inflight.next().await {
        if let Err(e) = result {
            super::emit_run_failed(name, &e.to_string());
        }
    }
}

/// One in-memory candidate: a message, the channel it came from, and the
/// surfacing source's priority bucket.
struct Cand {
    post: Post,
    channel_id: String,
    priority: Option<u64>,
}

async fn run_psyop(
    psyop: &PsyOp,
    name: &str,
    client: &discord::Client,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<(), Error> {
    // Every source's bot must be authed (skipped in mock mode). A missing
    // auth skips the whole psyop (exit code stays 0), mirroring X.
    if !ctx.config.mock {
        if let Some(agent_tag) = missing_agent_auth(ctx, psyop).await? {
            crate::output::OutputResult::from(crate::events::Event::PsyopAgentNotAuthed {
                psyop: name.to_string(),
                agent_tag,
            })
            .emit();
            return Ok(());
        }
    }

    // 1-2. Ingest + per-source python_filter.
    let mut cands: Vec<Cand> = Vec::new();
    for ch in psyop.channels.iter().flatten() {
        let label = format!("channel {}", ch.channel_id);
        let cid: ChannelId = match ch.channel_id.parse() {
            Ok(c) => c,
            Err(e) => {
                emit_source_result(name, &label, 0, Some(Error::Other(format!("invalid channel id: {e}"))));
                continue;
            }
        };
        // Auth errors surface as the first paged fetch's error.
        let (posts, err) = ingest_channel(&client, &ch.agent_tag, cid, ch.count).await;
        let kept = push_filtered(ctx, &mut cands, posts, ch.python_filter.as_deref(), ch.priority).await?;
        emit_source_result(name, &label, kept, err);
    }
    for sv in psyop.servers.iter().flatten() {
        let label = format!("server {}", sv.guild_id);
        let gid: GuildId = match sv.guild_id.parse() {
            Ok(g) => g,
            Err(e) => {
                emit_source_result(name, &label, 0, Some(Error::Other(format!("invalid guild id: {e}"))));
                continue;
            }
        };
        let (posts, err) = ingest_server(&client, &sv.agent_tag, gid, sv.count).await;
        let kept = push_filtered(ctx, &mut cands, posts, sv.python_filter.as_deref(), sv.priority).await?;
        emit_source_result(name, &label, kept, err);
    }

    // 3. Sort (priority bucket, then per-bucket SortBy).
    let ordered = sort_cands(ctx, &psyop.sort, cands).await?;

    // 4. Dedup by message id (keep first occurrence).
    let deduped = dedup_keep_first(ordered);

    // 5. Drop messages already delivered in a prior run.
    let pairs: Vec<(String, String)> = deduped
        .iter()
        .map(|c| (c.channel_id.clone(), c.post.id.clone()))
        .collect();
    let already = ctx.db.discord_already_delivered(name, &pairs).await?;
    let survivors_in: Vec<Cand> = deduped
        .into_iter()
        .filter(|c| !already.contains(&(c.channel_id.clone(), c.post.id.clone())))
        .collect();

    // message id → channel id (unique after dedup), needed for scoring input
    // and delivery.
    let channel_of: HashMap<String, String> = survivors_in
        .iter()
        .map(|c| (c.post.id.clone(), c.channel_id.clone()))
        .collect();
    let posts: Vec<Post> = survivors_in.into_iter().map(|c| c.post).collect();

    // 6. Score.
    let result = score_pipeline(psyop, name, posts, &channel_of, seed, ctx).await?;

    // 7. Deliver survivors to each configured agent + mark them delivered,
    //    concurrently (each failure emitted on its own; the rest still run).
    if !result.survivors.is_empty() && !psyop.agent_tags.is_empty() {
        use futures::future::FutureExt;
        let now = chrono::Utc::now().timestamp();
        // One id per run, stamped on every row it enqueues so readers can
        // group the run's messages. Random 128-bit hex.
        let run_id = format!("{:032x}", rand::random::<u128>());
        let survivors: Vec<(String, String, f64)> = result
            .survivors
            .iter()
            .map(|s| {
                let ch = channel_of.get(&s.post.id).cloned().unwrap_or_default();
                (ch, s.post.id.clone(), s.score)
            })
            .collect();
        let delivered_pairs: Vec<(String, String)> =
            survivors.iter().map(|(c, m, _)| (c.clone(), m.clone())).collect();

        let mut tasks: Vec<futures::future::BoxFuture<'_, Result<(), Error>>> = psyop
            .agent_tags
            .iter()
            .map(|agent_tag| deliver_to_agent(ctx, name, agent_tag, &survivors, &run_id, now).boxed())
            .collect();
        tasks.push(
            async {
                ctx.db
                    .discord_mark_delivered(name, &delivered_pairs)
                    .await
                    .map_err(Error::from)
            }
            .boxed(),
        );
        for r in futures::future::join_all(tasks).await {
            if let Err(e) = r {
                super::emit_run_failed(name, &e.to_string());
            }
        }
    }

    // Stamp the interval on success (harmless for manual triggers).
    ctx.db
        .set_last_run(name, chrono::Utc::now().timestamp())
        .await
        .map_err(Error::from)?;
    Ok(())
}

/// The first source `agent_tag` referenced by `psyop` that lacks a bot token,
/// or `None` if all are authed. Distinct tags only.
async fn missing_agent_auth(
    ctx: &crate::context::Context,
    psyop: &PsyOp,
) -> Result<Option<String>, Error> {
    let mut tags: Vec<&str> = Vec::new();
    for c in psyop.channels.iter().flatten() {
        if !tags.contains(&c.agent_tag.as_str()) {
            tags.push(&c.agent_tag);
        }
    }
    for s in psyop.servers.iter().flatten() {
        if !tags.contains(&s.agent_tag.as_str()) {
            tags.push(&s.agent_tag);
        }
    }
    for tag in tags {
        match ctx.db.discord_auth_get(tag).await? {
            Some(a) if a.bot_token.is_some() => {}
            _ => return Ok(Some(tag.to_string())),
        }
    }
    Ok(None)
}

/// Apply a source's `python_filter` (if any) to its freshly-ingested posts and
/// append the survivors to `cands`. Returns the kept count.
async fn push_filtered(
    ctx: &crate::context::Context,
    cands: &mut Vec<Cand>,
    posts: Vec<(Post, String)>,
    python_filter: Option<&str>,
    priority: Option<u64>,
) -> Result<usize, Error> {
    let mut kept = 0;
    for (post, channel_id) in posts {
        if let Some(src) = python_filter {
            if !filter_passes(ctx, src, &post, &channel_id).await? {
                continue;
            }
        }
        kept += 1;
        cands.push(Cand {
            post,
            channel_id,
            priority,
        });
    }
    Ok(kept)
}

async fn filter_passes(
    ctx: &crate::context::Context,
    src: &str,
    post: &Post,
    channel_id: &str,
) -> Result<bool, Error> {
    let result = crate::psyops::pyeval::run(ctx, src, message_json(post, channel_id)).await?;
    result
        .as_bool()
        .ok_or_else(|| Error::Other(format!("python_filter must return a bool, got {result}")))
}

// -- ingest ---------------------------------------------------------------

/// Page a single channel's history newest-first until `count` messages are
/// collected or the history runs out. Same partial-keep contract as the X
/// fetches: a page error stops the loop but keeps everything paged before it.
async fn ingest_channel(
    client: &discord::Client,
    agent_tag: &str,
    channel_id: ChannelId,
    count: u64,
) -> (Vec<(Post, String)>, Option<Error>) {
    let mut out: Vec<(Post, String)> = Vec::new();
    let mut before: Option<MessageId> = None;
    while (out.len() as u64) < count {
        // The cached method fetches full (100-message) pages; we over-fetch the
        // last page slightly and truncate at the end.
        let batch = match client
            .get_messages(agent_tag, GetMessages { channel: channel_id, before })
            .await
        {
            Ok(b) => b,
            Err(e) => return (out, Some(Error::Other(format!("get_messages: {e}")))),
        };
        if batch.is_empty() {
            break;
        }
        // Returned newest-first, so the last element is the oldest — the
        // cursor for the next (older) page.
        let last_id = batch.last().map(|m| m.id);
        for m in &batch {
            out.push((message_to_post(m), m.channel_id.to_string()));
        }
        match last_id {
            Some(id) => before = Some(id),
            None => break,
        }
    }
    out.truncate(count as usize);
    (out, None)
}

/// Page across a server's text channels, summing toward `count`. Reads only
/// top-level text-ish channels (archived threads are not fetched).
async fn ingest_server(
    client: &discord::Client,
    agent_tag: &str,
    guild_id: GuildId,
    count: u64,
) -> (Vec<(Post, String)>, Option<Error>) {
    let channels = match client.get_channels(agent_tag, guild_id).await {
        Ok(c) => c,
        Err(e) => return (Vec::new(), Some(Error::Other(format!("get_channels: {e}")))),
    };
    let mut out: Vec<(Post, String)> = Vec::new();
    for ch in channels {
        if (out.len() as u64) >= count {
            break;
        }
        if !is_text_channel(ch.kind) {
            continue;
        }
        let remaining = count - out.len() as u64;
        let (mut posts, err) = ingest_channel(client, agent_tag, ch.id, remaining).await;
        out.append(&mut posts);
        if let Some(e) = err {
            return (out, Some(e));
        }
    }
    (out, None)
}

/// Channel kinds that carry readable message history.
fn is_text_channel(kind: ChannelType) -> bool {
    matches!(
        kind,
        ChannelType::Text
            | ChannelType::News
            | ChannelType::PublicThread
            | ChannelType::PrivateThread
            | ChannelType::NewsThread
    )
}

/// A Discord message → the canonical [`Post`] shape. Attachments split into
/// images / videos by `content_type`; engagement metrics are 0 (Discord has
/// none). `created` is the RFC-3339 message timestamp.
fn message_to_post(m: &Message) -> Post {
    let mut images = Vec::new();
    let mut videos = Vec::new();
    for a in &m.attachments {
        let media = MediaUrl { url: a.url.clone() };
        match a.content_type.as_deref() {
            Some(ct) if ct.starts_with("image/") => images.push(media),
            Some(ct) if ct.starts_with("video/") => videos.push(media),
            _ => {} // unknown / non-media attachment — skip
        }
    }
    Post {
        id: m.id.to_string(),
        handle: m.author.name.clone(),
        text: m.content.clone(),
        images,
        videos,
        created: m.timestamp.to_string(),
        likes: 0,
        retweets: 0,
        replies: 0,
        impressions: 0,
    }
}

// -- sort -----------------------------------------------------------------

async fn sort_cands(
    ctx: &crate::context::Context,
    sort: &SortBy,
    cands: Vec<Cand>,
) -> Result<Vec<Cand>, Error> {
    let mut buckets: BTreeMap<u64, Vec<Cand>> = BTreeMap::new();
    let mut none_bucket: Vec<Cand> = Vec::new();
    for c in cands {
        match c.priority {
            Some(p) => buckets.entry(p).or_default().push(c),
            None => none_bucket.push(c),
        }
    }
    let mut out = Vec::new();
    for (_p, bucket) in buckets {
        out.extend(sort_bucket(ctx, sort, bucket).await?);
    }
    out.extend(sort_bucket(ctx, sort, none_bucket).await?);
    Ok(out)
}

async fn sort_bucket(
    ctx: &crate::context::Context,
    sort: &SortBy,
    mut bucket: Vec<Cand>,
) -> Result<Vec<Cand>, Error> {
    match sort {
        SortBy::Newest => {
            bucket.sort_by(|a, b| b.post.created.cmp(&a.post.created));
            Ok(bucket)
        }
        SortBy::Oldest => {
            bucket.sort_by(|a, b| a.post.created.cmp(&b.post.created));
            Ok(bucket)
        }
        SortBy::Python(src) => sort_python(ctx, src, bucket).await,
    }
}

/// Sort by the operator's Python expression: `input` is the message-dict list
/// (candidate order), the result is a positionally-aligned list of sort
/// values. Messages sort ascending by value; a `null` value (or a position
/// past a short list) drops that message; extras are ignored.
async fn sort_python(
    ctx: &crate::context::Context,
    src: &str,
    cands: Vec<Cand>,
) -> Result<Vec<Cand>, Error> {
    let input = serde_json::Value::Array(
        cands
            .iter()
            .map(|c| message_json(&c.post, &c.channel_id))
            .collect(),
    );
    let result = crate::psyops::pyeval::run(ctx, src, input).await?;
    let values = result
        .as_array()
        .ok_or_else(|| Error::Other("custom sort must return a list".into()))?;
    let mut keyed: Vec<(f64, Cand)> = Vec::with_capacity(cands.len());
    for (i, c) in cands.into_iter().enumerate() {
        let value = match values.get(i) {
            Some(v) => extract_sort_value(v)?,
            None => None,
        };
        if let Some(v) = value {
            keyed.push((v, c));
        }
    }
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(keyed.into_iter().map(|(_, c)| c).collect())
}

/// A custom-sort element: a number → its `f64` value; `null` → drop the item.
fn extract_sort_value(v: &serde_json::Value) -> Result<Option<f64>, Error> {
    if v.is_null() {
        return Ok(None);
    }
    if let Some(f) = v.as_f64() {
        return Ok(Some(f));
    }
    Err(Error::Other(format!(
        "custom sort values must be a number or null, got {v}"
    )))
}

/// Drop duplicate messages, keeping the first occurrence of each id.
fn dedup_keep_first(cands: Vec<Cand>) -> Vec<Cand> {
    let mut seen: HashSet<String> = HashSet::new();
    cands
        .into_iter()
        .filter(|c| seen.insert(c.post.id.clone()))
        .collect()
}

// -- score ----------------------------------------------------------------

struct ScoreResult {
    survivors: Vec<ScoredPost>,
}

async fn score_pipeline(
    psyop: &PsyOp,
    name: &str,
    posts: Vec<Post>,
    channel_of: &HashMap<String, String>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<ScoreResult, Error> {
    let mut current: Vec<Post> = posts;

    let stages: &[Stage] = psyop.stages.as_deref().unwrap_or(&[]);
    if stages.is_empty() {
        // No scoring → every survivor gets max score (1.0).
        let survivors: Vec<ScoredPost> = current
            .into_iter()
            .map(|post| ScoredPost { post, score: 1.0 })
            .collect();
        return Ok(ScoreResult { survivors });
    }

    let mut survivors: Vec<ScoredPost> = Vec::new();
    for (i, stage) in stages.iter().enumerate() {
        if current.is_empty() {
            crate::output::OutputResult::from(crate::events::Event::StageEmpty {
                psyop: name.to_string(),
                stage: i,
            })
            .emit();
            break;
        }
        crate::output::OutputResult::from(crate::events::Event::StageBegin { stage: i }).emit();

        let after_threshold: Vec<ScoredPost> = match stage {
            Stage::Bare { .. } => current
                .into_iter()
                .map(|post| ScoredPost { post, score: 1.0 })
                .collect(),
            Stage::Function {
                base: _,
                function,
                profile,
                strategy,
                invert,
                text,
                images,
                videos,
                output_threshold,
            } => {
                let scored = score_discord_function(
                    function, profile, strategy, *invert, *text, *images, *videos, current,
                    channel_of, seed, ctx,
                )
                .await?;
                match output_threshold {
                    Some(t) => scored.into_iter().filter(|s| s.score >= *t).collect(),
                    None => scored,
                }
            }
        };

        let after_top: Vec<ScoredPost> = match &stage.base().output_top {
            Some(OutputTop::Fraction(p)) if !after_threshold.is_empty() => {
                let n = ((after_threshold.len() as f64) * *p).ceil() as usize;
                after_threshold.into_iter().take(n).collect()
            }
            Some(OutputTop::Fixed(n)) => after_threshold.into_iter().take(*n as usize).collect(),
            Some(OutputTop::Python(src)) => {
                let n = output_top_python(ctx, src, &after_threshold, channel_of).await?;
                after_threshold.into_iter().take(n).collect()
            }
            _ => after_threshold,
        };

        survivors = after_top.clone();
        current = after_top.into_iter().map(|s| s.post).collect();

        crate::output::OutputResult::from(crate::events::Event::StageEnd { stage: i }).emit();
    }

    Ok(ScoreResult { survivors })
}

/// Score Discord messages with one function stage. Builds the per-message
/// input (`{ message_id, channel_id, text, images, videos }`), runs the
/// objectiveai execution, and returns `ScoredPost`s in score-descending order.
#[allow(clippy::too_many_arguments)]
async fn score_discord_function(
    function: &FullInlineFunctionOrRemoteCommitOptional,
    profile: &InlineProfileOrRemoteCommitOptional,
    strategy: &Strategy,
    invert: bool,
    text: bool,
    images: bool,
    videos: bool,
    posts: Vec<Post>,
    channel_of: &HashMap<String, String>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<Vec<ScoredPost>, Error> {
    let items: Vec<_> = posts
        .iter()
        .map(|p| {
            let channel_id = channel_of.get(&p.id).map(|s| s.as_str()).unwrap_or("");
            crate::input::new_discord_input_value(p, channel_id, text, images, videos)
        })
        .collect();
    let scores = score::score_items(function, profile, strategy, invert, items, seed, ctx).await?;

    let mut scored: Vec<ScoredPost> = posts
        .into_iter()
        .zip(scores)
        .map(|(post, score)| ScoredPost { post, score })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(scored)
}

/// Evaluate an `OutputTop::Python` cap over the scored messages: `input` is
/// the message-dict list plus each one's `score`.
async fn output_top_python(
    ctx: &crate::context::Context,
    src: &str,
    posts: &[ScoredPost],
    channel_of: &HashMap<String, String>,
) -> Result<usize, Error> {
    let input = serde_json::Value::Array(
        posts
            .iter()
            .map(|s| {
                let ch = channel_of.get(&s.post.id).map(|x| x.as_str()).unwrap_or("");
                message_with_score_json(&s.post, ch, s.score)
            })
            .collect(),
    );
    let result = crate::psyops::pyeval::run(ctx, src, input).await?;
    coerce_to_count(&result)
}

/// Accept a non-negative int directly, or a finite whole-valued non-negative
/// float. Rejects negatives, NaN/Inf, non-integral floats, and non-numerics.
fn coerce_to_count(v: &serde_json::Value) -> Result<usize, Error> {
    if let Some(u) = v.as_u64() {
        return usize::try_from(u)
            .map_err(|e| Error::Other(format!("output_top integer out of range: {e}")));
    }
    if let Some(f) = v.as_f64() {
        if !f.is_finite() || f < 0.0 || f.fract() != 0.0 {
            return Err(Error::Other(format!(
                "output_top returned {f} — must be a non-negative whole number"
            )));
        }
        return Ok(f as usize);
    }
    Err(Error::Other(format!(
        "output_top must return an int or whole-number float, got {v}"
    )))
}

// -- deliver --------------------------------------------------------------

/// Queue every survivor into one agent's Discord queue, then notify the agent.
async fn deliver_to_agent(
    ctx: &crate::context::Context,
    psyop: &str,
    agent_tag: &str,
    survivors: &[(String, String, f64)],
    run_id: &str,
    now: i64,
) -> Result<(), Error> {
    for (channel_id, message_id, score) in survivors {
        ctx.db
            .discord_queue_enqueue(&psychological_operations_db::DiscordQueueEntry {
                agent_tag: agent_tag.to_string(),
                channel_id: channel_id.clone(),
                message_id: message_id.clone(),
                psyop: Some(psyop.to_string()),
                score: Some(*score),
                deliverer_agent_instance_hierarchy: Some(
                    ctx.config.objectiveai_agent_instance_hierarchy.clone(),
                ),
                message: None,
                run_id: Some(run_id.to_string()),
                queued_at: now,
            })
            .await
            .map_err(Error::from)?;
    }
    crate::commands::agents::notify::notify_agent(&ctx.db, &ctx.executor, agent_tag).await
}

// -- json shapes ----------------------------------------------------------

/// The Discord-message dict handed to Python (`python_filter` / sort).
fn message_json(post: &Post, channel_id: &str) -> serde_json::Value {
    serde_json::json!({
        "message_id": post.id,
        "channel_id": channel_id,
        "handle": post.handle,
        "text": post.text,
        "created": post.created,
        "images": post.images.iter().map(|m| m.url.clone()).collect::<Vec<_>>(),
        "videos": post.videos.iter().map(|m| m.url.clone()).collect::<Vec<_>>(),
    })
}

/// [`message_json`] plus the just-computed `score` (for `output_top` Python).
fn message_with_score_json(post: &Post, channel_id: &str, score: f64) -> serde_json::Value {
    let mut v = message_json(post, channel_id);
    if let serde_json::Value::Object(map) = &mut v {
        map.insert("score".to_string(), serde_json::json!(score));
    }
    v
}

// -- events ---------------------------------------------------------------

/// Emit a per-source ingest result, reusing the X `QueryComplete`/`QueryFailed`
/// events with `query` carrying the source label (`"channel <id>"` etc.).
fn emit_source_result(name: &str, label: &str, count: usize, err: Option<Error>) {
    match err {
        None => crate::output::OutputResult::from(crate::events::Event::QueryComplete {
            psyop: name.to_string(),
            query: label.to_string(),
            count,
        })
        .emit(),
        Some(e) => crate::output::OutputResult::from(crate::events::Event::QueryFailed {
            psyop: name.to_string(),
            query: label.to_string(),
            error: format!("after {count} posts: {e}"),
        })
        .emit(),
    }
}
