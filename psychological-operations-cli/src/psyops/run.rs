//! `psyops run` — execute one or more psyops end-to-end, fully in memory.
//!
//! `run_all` resolves which psyops to run (an explicit `--name` list, or
//! every enabled psyop), drops any that fail validation / interval / agent
//! auth, then:
//!
//! * **Phase A — collect + hydrate for_you (sequential):** each *unique*
//!   for_you agent across the runnable set has its For You feed scraped
//!   ONCE (embedded browser, `AgentRead`), and each collected tweet ID is
//!   hydrated via the X API ONCE. The result (`agent → Vec<Post>`, arrival
//!   order) is shared by every psyop referencing that agent.
//! * **Phase B — score + deliver per psyop (parallel):** each psyop builds
//!   its candidate set from its for_you agents (+ its queries, scraped as
//!   `AuthMode::Agent`, run per the `query_when_for_you_queued` cost
//!   policy), filters, sorts (for_you interwoven across agents, ahead of
//!   query tweets), de-duplicates (keep first occurrence of each tweet),
//!   drops tweets it has already delivered in a prior run, scores, and
//!   delivers survivors to its `agent_tags` (recording them so they are
//!   never delivered again).
//!
//! If a psyop's scoring stages fail, its stage input is saved to the
//! `stage_retry` ledger and the run is NOT stamped; on the next run that
//! psyop skips Phase A and the whole candidate pipeline and re-scores the
//! saved input, clearing it on success.
//!
//! NOTHING about the candidate pipeline is persisted — posts, sources,
//! hydration, scores all live in memory for the lifetime of this call.
//! Only the per-psyop interval stamp (`psyop_runs`) and the delivered
//! survivors (agent `queue`) are durable.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use crate::db::{Origin, Post};
use crate::error::Error;
use crate::score::{self, ScoredPost};
use crate::tweet::Tweet;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::params::tweet_expansions_parameter::TweetExpansions;
use psychological_operations_sdk::x::params::tweet_fields_parameter::TweetFields;
use psychological_operations_sdk::x::params::user_fields_parameter::UserFields;
use psychological_operations_sdk::x::types::TweetId;

use super::{PsyOp, Query};

/// CLI entrypoint for `psyops::Commands::Run`.
pub async fn run_all(names: Vec<String>, seed: Option<i64>, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(run_all_inner(names, seed, ctx).await)
}

async fn run_all_inner(
    names: Vec<String>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let db = &ctx.db;

    // Load (name, PsyOp) pairs. Names given → load each by name (an
    // interval-blocked named psyop still announces the skip). No names →
    // every enabled psyop, interval-skipped silently.
    let (loaded, announce_interval): (Vec<(String, PsyOp)>, bool) = if names.is_empty() {
        let mut loaded = Vec::new();
        for (name, def, _disabled) in db
            .psyop_list()
            .await?
            .into_iter()
            .filter(|(_, _, disabled)| !disabled)
        {
            match serde_json::from_value::<PsyOp>(def) {
                Ok(psyop) => loaded.push((name, psyop)),
                Err(e) => emit_run_failed(&name, &e.to_string()),
            }
        }
        (loaded, false)
    } else {
        let mut loads: FuturesUnordered<_> = names
            .into_iter()
            .map(|name| async move {
                let result = super::psyop::load(&name, ctx).await;
                (name, result)
            })
            .collect();
        let mut loaded = Vec::new();
        while let Some((name, result)) = loads.next().await {
            match result {
                Ok(psyop) => loaded.push((name, psyop)),
                Err(e) => emit_run_failed(&name, &e.to_string()),
            }
        }
        (loaded, true)
    };

    // Resolve the runnable set: validate + interval gate + (for normal runs)
    // agent-auth gate. A psyop with a saved stage-retry input carries it and
    // takes the retry shortcut (skips collection/scrape, so no auth gate).
    // Each non-runnable psyop emits its own event and is dropped; only db
    // errors abort the batch.
    let mut runnable: Vec<(String, PsyOp, Option<Vec<Post>>)> = Vec::new();
    for (name, psyop) in loaded {
        if let Err(reason) = psyop.validate() {
            crate::output::OutputResult::from(crate::events::Event::PsyopInvalidAtRun {
                psyop: name.clone(),
                reason,
            })
            .emit();
            continue;
        }
        let interval = psyop.interval_duration().expect("validated interval");
        if let Some(last_run) = db.get_last_run(&name).await? {
            let elapsed = (chrono::Utc::now().timestamp() - last_run).max(0) as u64;
            if elapsed < interval.as_secs() {
                if announce_interval {
                    crate::output::OutputResult::from(crate::events::Event::PsyopSkippedInterval {
                        psyop: name.clone(),
                        interval: psyop.interval.clone(),
                        remaining_secs: interval.as_secs() - elapsed,
                    })
                    .emit();
                }
                continue;
            }
        }
        // A saved stage-retry input means: re-run the stages on it, skipping
        // everything up to them.
        let retry_input: Option<Vec<Post>> = match db.get_stage_retry(&name).await? {
            Some(v) => match serde_json::from_value::<Vec<Post>>(v) {
                Ok(posts) => Some(posts),
                Err(e) => {
                    emit_run_failed(&name, &format!("corrupt stage_retry: {e}"));
                    continue;
                }
            },
            None => None,
        };
        // Normal runs scrape, so every referenced agent must be authed
        // (skipped in mock mode, and for retry runs which don't scrape).
        if retry_input.is_none() && !ctx.config.mock {
            if let Some(agent_tag) = missing_agent_auth(db, &psyop).await? {
                crate::output::OutputResult::from(crate::events::Event::PsyopAgentNotAuthed {
                    psyop: name.clone(),
                    agent_tag,
                })
                .emit();
                continue;
            }
        }
        runnable.push((name, psyop, retry_input));
    }

    // ── Phase A — collect + hydrate for_you, once per unique agent ──────
    // Distinct for_you agents across the runnable set, in first-seen order.
    // Retry psyops don't collect — they re-score a saved input.
    let mut for_you_agents: Vec<String> = Vec::new();
    for (_, psyop, retry_input) in &runnable {
        if retry_input.is_some() {
            continue;
        }
        for fy in psyop.for_you.iter().flatten() {
            if !for_you_agents.contains(&fy.agent_tag) {
                for_you_agents.push(fy.agent_tag.clone());
            }
        }
    }

    let http = make_http_client(ctx);
    // Collect each agent's For You feed once (sequential browser). A failed
    // collection marks the agent so every psyop referencing it is dropped.
    let mut agent_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut failed_agents: HashSet<String> = HashSet::new();
    for agent in &for_you_agents {
        match super::collect::collect_for_you(agent, ctx).await {
            Ok(ids) => {
                agent_ids.insert(agent.clone(), ids);
            }
            Err(e) => {
                failed_agents.insert(agent.clone());
                for (name, psyop, retry_input) in &runnable {
                    if retry_input.is_none()
                        && psyop.for_you.iter().flatten().any(|fy| &fy.agent_tag == agent)
                    {
                        emit_run_failed(name, &format!("for_you collection ({agent}): {e}"));
                    }
                }
            }
        }
    }

    // Hydrate the union of collected IDs once (each tweet fetched a single
    // time even if several agents' feeds surfaced it), as the agent whose
    // feed surfaced it — the first agent (in collection order) to carry the
    // id does the fetch under its own auth.
    let mut hydrated: HashMap<String, Option<Post>> = HashMap::new();
    for agent in &for_you_agents {
        let Some(ids) = agent_ids.get(agent) else {
            continue;
        };
        crate::output::OutputResult::from(crate::events::Event::HydratingQueue {
            agent: agent.clone(),
            count: ids.len(),
        })
        .emit();
        let auth = AuthMode::Agent(agent.clone());
        for id in ids {
            if hydrated.contains_key(id) {
                continue;
            }
            match fetch_tweet(&http, &auth, id).await {
                Ok(Some(post)) => {
                    hydrated.insert(id.clone(), Some(post));
                }
                Ok(None) => {
                    crate::output::OutputResult::from(crate::events::Event::TweetNotFound {
                        agent: agent.clone(),
                        tweet_id: id.clone(),
                    })
                    .emit();
                    hydrated.insert(id.clone(), None);
                }
                Err(e) => {
                    crate::output::OutputResult::from(crate::events::Event::TweetFetchFailed {
                        agent: agent.clone(),
                        tweet_id: id.clone(),
                        error: e.to_string(),
                    })
                    .emit();
                    hydrated.insert(id.clone(), None);
                }
            }
        }
    }
    // agent → Vec<Post> in arrival order (drop not-found / failed fetches).
    let agent_posts: HashMap<String, Vec<Post>> = agent_ids
        .iter()
        .map(|(agent, ids)| {
            let posts = ids
                .iter()
                .filter_map(|id| hydrated.get(id).and_then(|o| o.clone()))
                .collect();
            (agent.clone(), posts)
        })
        .collect();

    // Drop psyops whose for_you collection failed (retry psyops never
    // collect, so they're never dropped here); the rest proceed.
    let ready: Vec<(String, PsyOp, Option<Vec<Post>>)> = runnable
        .into_iter()
        .filter(|(_, psyop, retry_input)| {
            retry_input.is_some()
                || !psyop
                    .for_you
                    .iter()
                    .flatten()
                    .any(|fy| failed_agents.contains(&fy.agent_tag))
        })
        .collect();

    // ── Phase B — score + deliver, concurrently ───────────────────────
    let mut inflight: FuturesUnordered<_> = ready
        .iter()
        .map(|(name, psyop, retry_input)| {
            let agent_posts = &agent_posts;
            let retry_input = retry_input.clone();
            async move {
                (
                    name.as_str(),
                    run_scored(psyop, name, agent_posts, retry_input, seed, ctx).await,
                )
            }
        })
        .collect();
    while let Some((name, result)) = inflight.next().await {
        if let Err(e) = result {
            emit_run_failed(name, &e.to_string());
        }
    }

    Ok(Output::Ok)
}

/// The first `agent_tag` referenced by `psyop`'s queries / for_you that
/// lacks valid auth (never logged in, or no tokens), or `None` if all are
/// authed. Distinct tags only.
async fn missing_agent_auth(
    db: &crate::db::Db,
    psyop: &PsyOp,
) -> Result<Option<String>, Error> {
    let mut tags: Vec<&str> = Vec::new();
    for q in psyop.queries.iter().flatten() {
        if !tags.contains(&q.agent_tag.as_str()) {
            tags.push(&q.agent_tag);
        }
    }
    for fy in psyop.for_you.iter().flatten() {
        if !tags.contains(&fy.agent_tag.as_str()) {
            tags.push(&fy.agent_tag);
        }
    }
    for tag in tags {
        // Persona kind is always "agent" now (accounts are agent-only).
        match db.persona_twid_get("agent", tag).await? {
            None => return Ok(Some(tag.to_string())),
            Some(twid) => {
                if db.account_auth_get(&twid).await?.is_none() {
                    return Ok(Some(tag.to_string()));
                }
            }
        }
    }
    Ok(None)
}

/// Emit a non-fatal per-psyop failure event (the batch keeps running).
fn emit_run_failed(psyop: &str, error: &str) {
    crate::output::OutputResult::from(crate::events::Event::PsyopRunFailed {
        psyop: psyop.to_string(),
        error: error.to_string(),
    })
    .emit();
}

/// Run one psyop: assemble candidates (for_you + conditional queries),
/// filter → sort → dedup → trim → score → deliver, then stamp the interval
/// on success. Pure in-memory; `agent_posts` is the shared, already-hydrated
/// for_you data. When `retry_input` is `Some`, everything up to the stages
/// is skipped and the stages re-run on that saved input.
async fn run_scored(
    psyop: &PsyOp,
    name: &str,
    agent_posts: &HashMap<String, Vec<Post>>,
    retry_input: Option<Vec<Post>>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    // The stage input: a saved retry input, or the full collect→…→trim
    // pipeline.
    let trimmed: Vec<Post> = match retry_input {
        Some(saved) => saved,
        None => {
            let http = make_http_client(ctx);

            // 1. Candidates from this psyop's for_you agents (config order).
            //    No de-dup: a tweet in two agents' feeds becomes two
            //    candidates.
            let mut cands: Vec<Cand> = Vec::new();
            for fy in psyop.for_you.iter().flatten() {
                let Some(posts) = agent_posts.get(&fy.agent_tag) else {
                    continue;
                };
                for (idx, post) in posts.iter().enumerate() {
                    cands.push(Cand {
                        post: post.clone(),
                        origin: Origin::ForYou(fy.agent_tag.clone()),
                        arrival: idx,
                    });
                }
            }
            let had_for_you = !cands.is_empty();

            // 2. Run queries (each as its own agent) per the cost policy.
            if psyop.query_when_for_you_queued || !had_for_you {
                run_queries_into(psyop, &http, name, &mut cands).await?;
            }

            // 3. Filter.
            let accepted = filter_with_priority(psyop, &cands)?;

            // 4. Priority-bucket sort (for_you interwoven, ahead of queries).
            let ordered = bucket_sort(psyop, accepted)?;

            // 5. De-duplicate (keep first occurrence), BEFORE the cap so
            //    max_posts counts distinct tweets.
            let deduped = dedup_keep_first(ordered.into_iter().map(|a| a.post).collect());

            // 5b. Drop tweets this psyop has already delivered in a prior run.
            let ids: Vec<String> = deduped.iter().map(|p| p.id.clone()).collect();
            let already = ctx
                .db
                .already_delivered(name, &ids)
                .await
                .map_err(Error::from)?;
            let deduped: Vec<Post> = deduped
                .into_iter()
                .filter(|p| !already.contains(&p.id))
                .collect();

            // 6. Trim to max_posts distinct tweets.
            deduped.into_iter().take(psyop.max_posts as usize).collect()
        }
    };

    // 7. Score. On stage-pipeline failure, persist the input for retry and
    //    bail WITHOUT stamping the run, so the next run re-scores it.
    let result = match score_pipeline(psyop, name, trimmed.clone(), seed, ctx).await {
        Ok(r) => r,
        Err(e) => {
            if let Ok(input) = serde_json::to_value(&trimmed) {
                // Best-effort: the stage error is the failure we report.
                let _ = ctx.db.save_stage_retry(name, &input).await;
            }
            return Err(e);
        }
    };

    // 8. Deliver survivors to each configured agent + mark them delivered,
    //    all concurrently. Deliveries do NOT cancel each other on the first
    //    failure (join_all, not try_join_all); each failure is emitted on
    //    its own and the rest still complete.
    if !result.survivors.is_empty() && !psyop.agent_tags.is_empty() {
        use futures::future::FutureExt;
        let now = chrono::Utc::now().timestamp();
        let survivors: Vec<(String, f64)> = result
            .survivors
            .iter()
            .map(|s| (s.post.id.clone(), s.score))
            .collect();
        let survivor_ids: Vec<String> = survivors.iter().map(|(id, _)| id.clone()).collect();

        let mut tasks: Vec<futures::future::BoxFuture<'_, Result<(), Error>>> = psyop
            .agent_tags
            .iter()
            .map(|agent_tag| deliver_to_agent(ctx, name, agent_tag, &survivors, now).boxed())
            .collect();
        // Mark every tweet output for delivery so this psyop never re-delivers it.
        tasks.push(
            async {
                ctx.db
                    .mark_delivered(name, &survivor_ids)
                    .await
                    .map_err(Error::from)
            }
            .boxed(),
        );

        for result in futures::future::join_all(tasks).await {
            if let Err(e) = result {
                emit_run_failed(name, &e.to_string());
            }
        }
    }

    // Stages succeeded — clear any pending retry, then stamp the interval.
    ctx.db.delete_stage_retry(name).await.map_err(Error::from)?;
    db_set_last_run(ctx, name).await?;
    Ok(Output::Ok)
}

async fn db_set_last_run(ctx: &crate::context::Context, name: &str) -> Result<(), Error> {
    ctx.db
        .set_last_run(name, chrono::Utc::now().timestamp())
        .await
        .map_err(|e| Error::Other(format!("set_last_run: {e}")))
}

/// One in-memory candidate occurrence for a psyop run. NO de-duplication:
/// a tweet surfaced by N sources (several agents' feeds and/or queries)
/// produces N independent candidates, each carrying a single origin, and
/// each flows through filter → sort → score → deliver on its own.
struct Cand {
    post: Post,
    /// The single source that surfaced this occurrence.
    origin: Origin,
    /// Arrival index in the agent's For You feed (for_you interweave);
    /// unused for query origins.
    arrival: usize,
}

/// Queue every survivor into one agent's queue, then notify the agent.
async fn deliver_to_agent(
    ctx: &crate::context::Context,
    psyop: &str,
    agent_tag: &str,
    survivors: &[(String, f64)],
    now: i64,
) -> Result<(), Error> {
    for (tweet_id, score) in survivors {
        ctx.db
            .queue_enqueue(&psychological_operations_db::QueueEntry {
                agent_tag: agent_tag.to_string(),
                tweet_id: tweet_id.clone(),
                psyop: Some(psyop.to_string()),
                score: Some(*score),
                deliverer_agent_instance_hierarchy: None,
                message: None,
                queued_at: now,
            })
            .await
            .map_err(Error::from)?;
    }
    crate::commands::agents::notify::notify_agent(ctx, agent_tag).await
}

// -- filter ---------------------------------------------------------------

/// An accepted candidate, ready to sort.
struct Accepted {
    tweet: Tweet,
    post: Post,
    /// Smallest `Some(_)` priority across accepting origins; `None` if no
    /// accepting origin had a priority.
    priority: Option<u64>,
    /// `(agent, arrival_index)` of the min-priority accepting for_you
    /// origin (ties → config order). `None` ⇒ accepted only via queries —
    /// sorts after for_you tweets in its bucket via `SortBy`.
    for_you: Option<(String, usize)>,
}

fn filter_with_priority(psyop: &PsyOp, cands: &[Cand]) -> Result<Vec<Accepted>, Error> {
    let now = chrono::Utc::now();
    let mut out = Vec::new();
    for cand in cands {
        // Each candidate carries exactly one origin (no de-dup).
        let (filter, priority) = match origin_lookup(psyop, &cand.origin) {
            Some(p) => p,
            None => continue, // origin no longer present in psyop config
        };
        let tweet = tweet_from_post(&cand.post, &now);
        let passes = match filter {
            Some(f) => crate::psyops::filter::evaluate(f, &tweet).map_err(Error::Other)?,
            None => true,
        };
        if !passes {
            continue;
        }
        let for_you = match &cand.origin {
            Origin::ForYou(agent) => Some((agent.clone(), cand.arrival)),
            Origin::Query(_) => None,
        };
        out.push(Accepted {
            tweet,
            post: cand.post.clone(),
            priority,
            for_you,
        });
    }
    Ok(out)
}

fn origin_lookup<'a>(
    psyop: &'a PsyOp,
    origin: &Origin,
) -> Option<(Option<&'a crate::psyops::Filter>, Option<u64>)> {
    match origin {
        Origin::ForYou(agent) => {
            let fy = psyop
                .for_you
                .iter()
                .flatten()
                .find(|fy| &fy.agent_tag == agent)?;
            Some((fy.filter.as_ref(), fy.priority))
        }
        Origin::Query(q) => {
            let qs = psyop.queries.as_ref()?;
            let matched: &Query = qs.iter().find(|qq| qq.query == *q)?;
            Some((matched.filter.as_ref(), matched.priority))
        }
    }
}

// -- sort -----------------------------------------------------------------

fn bucket_sort(psyop: &PsyOp, accepted: Vec<Accepted>) -> Result<Vec<Accepted>, Error> {
    let mut buckets: BTreeMap<u64, Vec<Accepted>> = BTreeMap::new();
    let mut none_bucket: Vec<Accepted> = Vec::new();
    for a in accepted {
        match a.priority {
            Some(p) => buckets.entry(p).or_default().push(a),
            None => none_bucket.push(a),
        }
    }
    let mut out = Vec::new();
    for (_p, bucket) in buckets {
        out.extend(sort_bucket(psyop, bucket)?);
    }
    out.extend(sort_bucket(psyop, none_bucket)?);
    Ok(out)
}

/// Within one priority bucket: for_you tweets first (interwoven across
/// agents), then query-only tweets ordered by the psyop's `SortBy`.
fn sort_bucket(psyop: &PsyOp, bucket: Vec<Accepted>) -> Result<Vec<Accepted>, Error> {
    let (fy, qs): (Vec<Accepted>, Vec<Accepted>) =
        bucket.into_iter().partition(|a| a.for_you.is_some());

    // for_you: group by collecting agent (config order), each group in
    // arrival order, then round-robin merge (a₁,b₁,c₁,a₂,…).
    let agent_order: Vec<String> = {
        let mut order = Vec::new();
        for fent in psyop.for_you.iter().flatten() {
            if !order.contains(&fent.agent_tag) {
                order.push(fent.agent_tag.clone());
            }
        }
        order
    };
    let mut groups: HashMap<String, Vec<Accepted>> = HashMap::new();
    for a in fy {
        let agent = a.for_you.as_ref().map(|(ag, _)| ag.clone()).unwrap_or_default();
        groups.entry(agent).or_default().push(a);
    }
    let mut queues: Vec<VecDeque<Accepted>> = agent_order
        .iter()
        .filter_map(|agent| groups.remove(agent))
        .map(|mut g| {
            g.sort_by_key(|a| a.for_you.as_ref().map(|(_, i)| *i).unwrap_or(usize::MAX));
            VecDeque::from(g)
        })
        .collect();
    let mut fy_ordered = Vec::new();
    loop {
        let mut progressed = false;
        for q in queues.iter_mut() {
            if let Some(a) = q.pop_front() {
                fy_ordered.push(a);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    // query-only: sort the Accepted directly by the psyop's SortBy (the
    // evaluator reads each one's tweet — no tweet-ID round-trip). A Custom
    // sort may also drop entries (None values), so this can be shorter.
    let q_ordered =
        crate::psyops::sort_by::evaluate(&psyop.sort, qs, |a| &a.tweet).map_err(Error::Other)?;

    let mut out = fy_ordered;
    out.extend(q_ordered);
    Ok(out)
}

fn tweet_from_post(p: &Post, now: &chrono::DateTime<chrono::Utc>) -> Tweet {
    Tweet {
        id: p.id.clone(),
        handle: p.handle.clone(),
        age: crate::db::compute_age(&p.created, now),
        created: p.created.clone(),
        likes: p.likes,
        retweets: p.retweets,
        replies: p.replies,
        impressions: p.impressions,
    }
}

/// Drop duplicate tweets, keeping the first occurrence of each ID (its
/// best-ranked position in the ordered list) and removing every later copy.
fn dedup_keep_first(posts: Vec<Post>) -> Vec<Post> {
    let mut seen: HashSet<String> = HashSet::new();
    posts.into_iter().filter(|p| seen.insert(p.id.clone())).collect()
}

// -- queries --------------------------------------------------------------

/// Run each of `psyop`'s queries (as its own agent) and merge the results
/// into `cands` with a `Query` origin. Endpoints other than `recent` are
/// skipped with a notice.
async fn run_queries_into(
    psyop: &PsyOp,
    http: &Client,
    name: &str,
    cands: &mut Vec<Cand>,
) -> Result<(), Error> {
    let queries = match &psyop.queries {
        Some(qs) if !qs.is_empty() => qs,
        _ => return Ok(()),
    };
    for q in queries {
        let auth = AuthMode::Agent(q.agent_tag.clone());
        match search_recent(http, &auth, &q.query).await {
            Ok(posts) => {
                crate::output::OutputResult::from(crate::events::Event::QueryComplete {
                    psyop: name.to_string(),
                    query: q.query.clone(),
                    count: posts.len(),
                })
                .emit();
                for p in posts {
                    cands.push(Cand {
                        post: p,
                        origin: Origin::Query(q.query.clone()),
                        arrival: 0,
                    });
                }
            }
            Err(e) => {
                crate::output::OutputResult::from(crate::events::Event::QueryFailed {
                    psyop: name.to_string(),
                    query: q.query.clone(),
                    error: e.to_string(),
                })
                .emit();
            }
        }
    }
    Ok(())
}

// -- score pipeline -------------------------------------------------------

/// Output of `score_pipeline` — every post that got a score, plus the
/// final survivors of all stages.
struct ScoreResult {
    survivors: Vec<ScoredPost>,
}

async fn score_pipeline(
    psyop: &PsyOp,
    name: &str,
    posts: Vec<Post>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<ScoreResult, Error> {
    let mut current: Vec<Post> = posts;

    // No scoring stages → every survivor gets max score (1.0).
    let stages: &[crate::psyops::Stage] = psyop.stages.as_deref().unwrap_or(&[]);
    if stages.is_empty() {
        const MAX_SCORE: f64 = 1.0;
        let survivors: Vec<ScoredPost> = current
            .into_iter()
            .map(|post| ScoredPost {
                post,
                score: MAX_SCORE,
            })
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

        let (_scored, after_threshold) = match stage {
            crate::psyops::Stage::Bare { .. } => {
                let scored: Vec<ScoredPost> = current
                    .into_iter()
                    .map(|post| ScoredPost { post, score: 1.0 })
                    .collect();
                let passthrough = scored.clone();
                (scored, passthrough)
            }
            crate::psyops::Stage::Function {
                base: _,
                function,
                profile,
                strategy,
                invert,
                images,
                videos,
                output_threshold,
            } => {
                let scored: Vec<ScoredPost> = score::score_function(
                    function, profile, strategy, *invert, *images, *videos, current, seed, ctx,
                )
                .await?;
                let after_threshold: Vec<ScoredPost> = match output_threshold {
                    Some(t) => scored.iter().cloned().filter(|s| s.score >= *t).collect(),
                    None => scored.clone(),
                };
                (scored, after_threshold)
            }
        };

        let after_top: Vec<ScoredPost> = match &stage.base().output_top {
            Some(crate::psyops::OutputTop::Fraction(p)) if !after_threshold.is_empty() => {
                let n = ((after_threshold.len() as f64) * *p).ceil() as usize;
                after_threshold.into_iter().take(n).collect()
            }
            Some(crate::psyops::OutputTop::Fixed(n)) => {
                after_threshold.into_iter().take(*n as usize).collect()
            }
            Some(crate::psyops::OutputTop::Starlark(src)) => {
                let n = super::output_top::evaluate(src, &after_threshold)
                    .map_err(crate::error::Error::Other)?;
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

// -- X API ----------------------------------------------------------------

fn make_http_client(ctx: &crate::context::Context) -> Client {
    Client::new(
        reqwest::Client::new(),
        ctx.config.mock,
        ctx.cache_max_size,
        ctx.cache_ttl,
        ctx.config.state_dir(),
        ctx.db.clone(),
    )
}

fn standard_tweet_fields() -> Vec<TweetFields> {
    vec![
        TweetFields::CreatedAt,
        TweetFields::PublicMetrics,
        TweetFields::AuthorId,
    ]
}

async fn fetch_tweet(http: &Client, auth: &AuthMode, id: &str) -> Result<Option<Post>, Error> {
    use psychological_operations_sdk::x::tweets::id::get;
    use psychological_operations_sdk::x::tweets::id::http::get as call;
    let req = get::Request {
        id: TweetId(id.to_string()),
        tweet_fields: Some(standard_tweet_fields()),
        expansions: Some(vec![TweetExpansions::AuthorId]),
        user_fields: Some(vec![UserFields::Username]),
        ..default_id_request()
    };
    let resp = call(http, auth, &req)
        .await
        .map_err(|e| Error::Other(format!("X /2/tweets/{id} failed: {e}")))?;
    let tweet = match resp.data {
        Some(t) => t,
        None => return Ok(None),
    };
    Ok(Some(tweet_to_post(&tweet, resp.includes.as_ref())))
}

async fn search_recent(http: &Client, auth: &AuthMode, query: &str) -> Result<Vec<Post>, Error> {
    use psychological_operations_sdk::x::tweets::search::recent::get;
    use psychological_operations_sdk::x::tweets::search::recent::http::get as call;
    let req = get::Request {
        query: query.to_string(),
        tweet_fields: Some(standard_tweet_fields()),
        expansions: Some(vec![TweetExpansions::AuthorId]),
        user_fields: Some(vec![UserFields::Username]),
        max_results: Some(100),
        ..default_recent_request()
    };
    let resp = call(http, auth, &req)
        .await
        .map_err(|e| Error::Other(format!("X /2/tweets/search/recent failed: {e}")))?;
    let tweets = resp.data.unwrap_or_default();
    Ok(tweets
        .iter()
        .map(|t| tweet_to_post(t, resp.includes.as_ref()))
        .collect())
}

fn tweet_to_post(
    t: &psychological_operations_sdk::x::types::Tweet,
    includes: Option<&psychological_operations_sdk::x::types::Expansions>,
) -> Post {
    let id = t.id.as_ref().map(|i| i.0.clone()).unwrap_or_default();
    let handle = lookup_handle(t, includes);
    let created = t.created_at.map(|d| d.to_rfc3339()).unwrap_or_default();
    let (likes, retweets, replies, impressions) = match &t.public_metrics {
        Some(m) => (
            m.like_count as u64,
            m.retweet_count as u64,
            m.reply_count as u64,
            m.impression_count as u64,
        ),
        None => (0, 0, 0, 0),
    };
    let text = t.text.as_ref().map(|tt| tt.0.clone()).unwrap_or_default();
    Post {
        id,
        handle,
        text,
        images: Vec::new(),
        videos: Vec::new(),
        created,
        likes,
        retweets,
        replies,
        impressions,
    }
}

fn lookup_handle(
    t: &psychological_operations_sdk::x::types::Tweet,
    includes: Option<&psychological_operations_sdk::x::types::Expansions>,
) -> String {
    let author_id = match &t.author_id {
        Some(a) => &a.0,
        None => return String::new(),
    };
    let users = match includes.and_then(|i| i.users.as_ref()) {
        Some(u) => u,
        None => return String::new(),
    };
    users
        .iter()
        .find(|u| u.id.0 == *author_id)
        .map(|u| u.username.0.clone())
        .unwrap_or_default()
}

// -- glue -----------------------------------------------------------------

fn default_id_request() -> psychological_operations_sdk::x::tweets::id::get::Request {
    use psychological_operations_sdk::x::tweets::id::get::Request;
    use psychological_operations_sdk::x::types::TweetId;
    Request {
        id: TweetId(String::new()),
        tweet_fields: None,
        expansions: None,
        media_fields: None,
        poll_fields: None,
        user_fields: None,
        place_fields: None,
    }
}

fn default_recent_request() -> psychological_operations_sdk::x::tweets::search::recent::get::Request
{
    use psychological_operations_sdk::x::tweets::search::recent::get::Request;
    Request {
        query: String::new(),
        start_time: None,
        end_time: None,
        since_id: None,
        until_id: None,
        max_results: None,
        next_token: None,
        pagination_token: None,
        sort_order: None,
        tweet_fields: None,
        expansions: None,
        media_fields: None,
        poll_fields: None,
        user_fields: None,
        place_fields: None,
    }
}
