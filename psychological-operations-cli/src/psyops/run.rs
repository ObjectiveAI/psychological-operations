//! `psyops run` — execute one or more psyops end-to-end.
//!
//! `run_all` resolves which psyops to run (an explicit `--name` list, or
//! every enabled psyop), drops any whose `interval` hasn't elapsed, runs
//! the interactive `for_you` browser collection **sequentially**, then
//! runs each psyop's scoring/delivery (`run_scored`) **in parallel**.
//!
//! Per-psyop flow (`run_scored`):
//! 1. Drain the for_you_queue, hydrating each id via X v2 `/2/tweets/{id}`
//!    and persisting via `Db::insert_post(_, _, _, Origin::ForYou)`.
//! 2. Read every unscored tweet for `(psyop, commit)` with its origins.
//! 3. Filter — accept iff at least one origin's filter accepts; the
//!    tweet's effective priority is the smallest priority across
//!    accepting origins.
//! 4. If filtered count < `min_posts` and queries haven't run yet
//!    (and the for_you_queued policy allows), run the psyop's queries
//!    via X v2 `/2/tweets/search/recent`, persist results, loop back
//!    to step 1.
//! 5. Bucket-sort accepted tweets by effective priority (smallest
//!    first; `None` last); each bucket is sorted via `SortBy::evaluate`;
//!    buckets concatenate in priority order.
//! 6. Trim to `max_posts`.
//! 7. Run multi-stage scoring (objectiveai), capturing every scored
//!    post + the final survivors.
//! 8. Persist scores via `Db::set_scores`.
//! 9. Reap `contents` for every post under (psyop, commit) so storage
//!    doesn't accumulate (`Db::drop_psyop_contents`).
//! 10. Enqueue one `delivery_queue` row per (applicable target,
//!     final-survivors) tuple — global + per-psyop targets.
//! 11. Drain the delivery queue via `targets::drain_queue` (filtered
//!     to this psyop).

use std::collections::{BTreeMap, HashMap};

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use crate::db::{Db, Origin, Post};
use crate::error::Error;
use crate::score::{self, ScoredPost};
use crate::tweet::Tweet;
use psychological_operations_sdk::cli::Output;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::params::tweet_expansions_parameter::TweetExpansions;
use psychological_operations_sdk::x::params::tweet_fields_parameter::TweetFields;
use psychological_operations_sdk::x::params::user_fields_parameter::UserFields;
use psychological_operations_sdk::x::types::TweetId;

use crate::psyops::SearchEndpoint;
use super::{PsyOp, Query};

/// CLI entrypoint for `psyops::Commands::Run`.
///
/// `names` selects which psyops to run: a non-empty list runs exactly
/// those; an empty list runs every enabled psyop. Either way a psyop only
/// runs if its `interval` has elapsed since its last successful run.
///
/// Two phases: the interactive `for_you` browser collection runs
/// **sequentially** (one CEF browser at a time), then — once every
/// selected psyop has collected — the scoring/delivery runs **in
/// parallel**.
pub async fn run_all(
    names: Vec<String>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(run_all_inner(names, seed, ctx).await)
}

async fn run_all_inner(
    names: Vec<String>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let db = &ctx.db;

    // Load the (name, PsyOp) pairs, and decide whether an interval skip
    // is announced:
    //
    // * Names given → load each named psyop DIRECTLY and in parallel —
    //   exact name matches only. An interval-blocked named psyop still
    //   emits `PsyopSkippedInterval`: you asked for it by name, so you
    //   hear that it isn't due yet.
    // * No names → fetch every enabled psyop in one shot. Psyops outside
    //   their interval are skipped SILENTLY — "run whatever can run".
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

    // Resolve the runnable set: validate + X-app preflight + interval
    // gate. Each non-runnable psyop emits its own event (except a silent
    // interval skip in the no-names case) and is dropped; only db errors
    // abort the batch.
    let mut runnable: Vec<(String, PsyOp)> = Vec::new();
    for (name, psyop) in loaded {
        if let Err(reason) = psyop.validate() {
            // Invalid psyop at run-time → skip + warn, not a failure.
            crate::output::OutputResult::from(crate::events::Event::PsyopInvalidAtRun {
                psyop: name.clone(),
                reason,
            })
            .emit();
            continue;
        }
        // X-app preflight — every psyop hits the real X API.
        if let Err(e) =
            psychological_operations_sdk::x::x_app::config::ensure_setup(db).await
        {
            emit_run_failed(&name, &format!("x_app: {e}"));
            continue;
        }
        // Interval gate — applies to explicitly-named psyops too: naming
        // a psyop never bypasses its throttle. validate() guarantees the
        // parse. The skip is announced only when names were given.
        let interval = psyop.interval_duration().expect("validated interval");
        if let Some(last_run) = db.get_last_run(&name).await? {
            let elapsed = (chrono::Utc::now().timestamp() - last_run).max(0) as u64;
            if elapsed < interval.as_secs() {
                if announce_interval {
                    crate::output::OutputResult::from(
                        crate::events::Event::PsyopSkippedInterval {
                            psyop: name.clone(),
                            interval: psyop.interval.clone(),
                            remaining_secs: interval.as_secs() - elapsed,
                        },
                    )
                    .emit();
                }
                continue;
            }
        }
        runnable.push((name, psyop));
    }

    // Phase A — `for_you` collection, strictly SEQUENTIAL: it drives the
    // single CEF browser and blocks on the operator closing each window.
    // A collect failure drops that psyop from the run set (per-psyop,
    // non-fatal).
    let mut ready: Vec<(String, PsyOp)> = Vec::with_capacity(runnable.len());
    for (name, psyop) in runnable {
        if psyop.for_you.is_some() {
            if let Err(e) = super::collect::collect_for_you(db, &name, ctx).await {
                emit_run_failed(&name, &e.to_string());
                continue;
            }
        }
        ready.push((name, psyop));
    }

    // Phase B — every collected psyop scores + delivers CONCURRENTLY;
    // surface each hard failure as it completes without aborting the
    // others.
    let mut inflight: FuturesUnordered<_> = ready
        .iter()
        .map(|(name, psyop)| async move {
            (name.as_str(), run_scored(psyop, name, seed, ctx).await)
        })
        .collect();
    while let Some((name, result)) = inflight.next().await {
        if let Err(e) = result {
            emit_run_failed(name, &e.to_string());
        }
    }

    Ok(Output::Ok)
}

/// Emit a non-fatal per-psyop failure event (the batch keeps running).
fn emit_run_failed(psyop: &str, error: &str) {
    crate::output::OutputResult::from(crate::events::Event::PsyopRunFailed {
        psyop: psyop.to_string(),
        error: error.to_string(),
    })
    .emit();
}

/// Run one already-collected, interval-cleared psyop: hydrate → filter →
/// (queries if short) → sort → score → persist → enqueue → deliver, then
/// stamp the interval throttle on success. This is the parallel unit of
/// `psyops run` (Phase B).
async fn run_scored(
    psyop: &PsyOp,
    name: &str,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<Output, Error> {
    let db = &ctx.db;
    let http = make_http_client(ctx);
    // Every X API call in this pipeline acts as the master X-App.
    let auth = AuthMode::XApp;

    // Capture whether the for_you_queue was non-empty at run start —
    // the `query_when_for_you_queued` policy reads this on the
    // re-loop iteration to decide whether queries are allowed.
    // When `for_you` is unconfigured, there's no queue to check
    // and the policy becomes a no-op.
    let had_for_you_queued_at_start = match &psyop.for_you {
        Some(_) => !db.for_you_queue(name).await?.is_empty(),
        None    => false,
    };
    let mut queries_already_ran = false;

    loop {
        // 1. Hydrate the for-you queue (drains everything currently
        //    in it). Skipped entirely when `for_you` is unconfigured.
        if psyop.for_you.is_some() {
            hydrate_for_you(db, &http, &auth, name).await?;
        }

        // 2. Read unscored tweets for this psyop, mapping each row to
        //    the runtime `Tweet` (computing `age` against `now`).
        let now = chrono::Utc::now();
        let rows = db.list_unscored_with_origins(name).await?;
        let entries: Vec<(Tweet, Vec<Origin>, i64)> = rows
            .into_iter()
            .map(|(row, origins)| {
                let tweet = Tweet {
                    id: row.id,
                    handle: row.handle,
                    age: crate::db::compute_age(&row.created, &now),
                    created: row.created,
                    likes: row.likes,
                    retweets: row.retweets,
                    replies: row.replies,
                    impressions: row.impressions,
                };
                (tweet, origins, row.seq)
            })
            .collect();

        // 3. Filter with priority resolution.
        let accepted = filter_with_priority(psyop, entries)?;

        // 4. Eligibility — run queries if we're short.
        if (accepted.len() as u64) < psyop.min_posts {
            if queries_already_ran {
                return Err(Error::Other(format!(
                    "psyop \"{name}\": only {} accepted after running queries; min_posts is {}",
                    accepted.len(), psyop.min_posts,
                )));
            }
            if !psyop.query_when_for_you_queued && had_for_you_queued_at_start {
                return Err(Error::Other(format!(
                    "psyop \"{name}\": only {} accepted; queries skipped because for_you queue was non-empty at start and query_when_for_you_queued = false",
                    accepted.len(),
                )));
            }
            run_queries(psyop, db, &http, &auth, name).await?;
            queries_already_ran = true;
            continue;
        }

        // 5. Priority-bucket sort.
        let final_list = bucket_sort(psyop, accepted)?;

        // 6. Trim to max_posts.
        let trimmed: Vec<Tweet> = final_list
            .into_iter()
            .take(psyop.max_posts as usize)
            .collect();

        // 7. Hydrate Tweet -> Post by joining with the `contents`
        //    table, then run the multi-stage scoring pipeline.
        let result = score_pipeline(db, psyop, name, trimmed, seed, ctx).await?;

        // 8. Persist scores for every scored post.
        if !result.last_scores.is_empty() {
            let ids: Vec<String> = result.last_scores.keys().cloned().collect();
            let scores: Vec<f64> = ids.iter().map(|id| result.last_scores[id]).collect();
            db.set_scores(&ids, &scores).await?;
        }

        // 9. Reap content for every post under `name`, scored or not.
        let _dropped = db.drop_psyop_contents(name).await?;

        // 10. Enqueue a delivery_queue row per (target, survivors).
        //     Targets are stored as JSONB; pass them straight through.
        if !result.survivors.is_empty() {
            let post_ids: Vec<String> = result.survivors.iter()
                .map(|s| s.post.id.clone())
                .collect();
            let post_ids_value = serde_json::to_value(&post_ids)?;

            let mut targets = db.global_targets().await?;
            targets.extend(db.psyop_targets(name).await?);
            for target in &targets {
                db.enqueue_delivery(name, target, &post_ids_value).await?;
            }
        }

        // 11. Drain the queue (narrowed to exactly this psyop — the
        //     rows we just enqueued).
        let _summary = crate::targets::drain_queue(db, Some(name), ctx).await?;

        // Stamp the interval throttle only on success — a failed
        // run bails via `?` above and stays immediately retryable.
        db.set_last_run(name, chrono::Utc::now().timestamp()).await?;

        return Ok(Output::Ok);
    }
}

/// Output of `score_pipeline` — every post that got a score, plus the
/// final survivors of all stages (which are what targets fire against).
struct ScoreResult {
    last_scores: HashMap<String, f64>,
    survivors:   Vec<ScoredPost>,
}

// -- step 7: score pipeline -----------------------------------------------

async fn score_pipeline(
    db: &Db,
    psyop: &PsyOp,
    name: &str,
    trimmed: Vec<Tweet>,
    seed: Option<i64>,
    ctx: &crate::context::Context,
) -> Result<ScoreResult, Error> {
    // Hydrate Tweet -> Post via contents lookup. Tweets whose
    // contents row is absent are filtered out — by contract those
    // posts don't exist for our purposes.
    let ids: Vec<String> = trimmed.iter().map(|t| t.id.clone()).collect();
    let contents = db.fetch_contents(&ids).await?;
    let mut current: Vec<Post> = trimmed
        .into_iter()
        .filter_map(|t| {
            let (text, images, videos) = contents.get(&t.id)?.clone();
            Some(Post {
                id: t.id,
                handle: t.handle,
                text,
                images,
                videos,
                created: t.created,
                likes: t.likes,
                retweets: t.retweets,
                replies: t.replies,
                impressions: t.impressions,
            })
        })
        .collect();

    // No scoring stages → every survivor gets max score (1.0).
    // No StageBegin/StageEnd events fire; the delivery path sees
    // a clean Vec<ScoredPost> exactly as if a single perfect
    // stage had run.
    let stages: &[crate::psyops::Stage] = psyop.stages.as_deref().unwrap_or(&[]);
    if stages.is_empty() {
        const MAX_SCORE: f64 = 1.0;
        let last_scores: HashMap<String, f64> = current
            .iter().map(|p| (p.id.clone(), MAX_SCORE)).collect();
        let survivors: Vec<ScoredPost> = current
            .into_iter()
            .map(|post| ScoredPost { post, score: MAX_SCORE })
            .collect();
        return Ok(ScoreResult { last_scores, survivors });
    }

    // Each post's score = the LAST stage that scored it. Survivors
    // of every stage end up with the final stage's score; posts
    // dropped at stage K end up with stage K's score.
    let mut last_scores: HashMap<String, f64> = HashMap::new();
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

        // Bracket each stage with marker notifications so consumers
        // can see exactly where one stage ends and the next begins in
        // the JSONL stream. Snapshot wire shape (after host re-wrap):
        //   {"type":"notification","value":{"event":"stage_begin","stage":N}}
        //   …per-stage scoring notifications…
        //   {"type":"notification","value":{"event":"stage_end","stage":N}}
        crate::output::OutputResult::from(crate::events::Event::StageBegin { stage: i }).emit();

        // Variant-specific scoring + per-variant narrowing.
        // `Stage::Bare` skips both objectiveai and threshold —
        // every post is a flat 1.0, then `output_top` (if set)
        // applies. `Stage::Function` does the full
        // function-call → threshold → top dance.
        let (scored, after_threshold) = match stage {
            crate::psyops::Stage::Bare { .. } => {
                let scored: Vec<ScoredPost> = current
                    .into_iter()
                    .map(|post| ScoredPost { post, score: 1.0 })
                    .collect();
                let passthrough = scored.clone();
                (scored, passthrough)
            }
            crate::psyops::Stage::Function {
                base: _, function, profile, strategy,
                invert, images, videos, output_threshold,
            } => {
                let scored: Vec<ScoredPost> = score::score_function(
                    function, profile, strategy,
                    *invert, *images, *videos,
                    current, seed, ctx,
                ).await?;
                // output_threshold: drop scores < threshold.
                let after_threshold: Vec<ScoredPost> = match output_threshold {
                    Some(t) => scored.iter().cloned().filter(|s| s.score >= *t).collect(),
                    None    => scored.clone(),
                };
                (scored, after_threshold)
            }
        };
        for s in &scored {
            last_scores.insert(s.post.id.clone(), s.score);
        }

        // output_top: keep top N (Fixed) or top ceil(N · pct)
        // (Fraction). Lives on the shared StageBase, so it's
        // applied for both variants.
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

    Ok(ScoreResult { last_scores, survivors })
}

// -- step 1: hydrate -------------------------------------------------------

async fn hydrate_for_you(
    db: &Db,
    http: &Client,
    auth: &AuthMode,
    name: &str,
) -> Result<(), Error> {
    let queued = db.for_you_queue(name).await?;
    if queued.is_empty() {
        return Ok(());
    }
    crate::output::OutputResult::from(crate::events::Event::HydratingQueue {
        psyop: name.to_string(),
        count: queued.len(),
    })
    .emit();
    let mut succeeded: Vec<String> = Vec::new();
    for id in queued {
        match fetch_tweet(http, auth, &id).await {
            Ok(Some(post)) => {
                db.insert_post(&post, name, &Origin::ForYou).await?;
                succeeded.push(id);
            }
            Ok(None) => {
                crate::output::OutputResult::from(crate::events::Event::TweetNotFound {
                    psyop: name.to_string(),
                    tweet_id: id.clone(),
                })
                .emit();
                succeeded.push(id);   // unrecoverable — don't keep retrying
            }
            Err(e) => {
                crate::output::OutputResult::from(crate::events::Event::TweetFetchFailed {
                    psyop: name.to_string(),
                    tweet_id: id,
                    error: e.to_string(),
                })
                .emit();
                // leave in queue for next round
            }
        }
    }
    db.dequeue_for_you(name, &succeeded).await?;
    Ok(())
}

// -- step 3: filter --------------------------------------------------------

struct Accepted {
    tweet: Tweet,
    /// Smallest `Some(_)` priority across this tweet's accepting
    /// origins; `None` if no accepting origin had a priority set.
    priority: Option<u64>,
    /// `posts.rowid` for this tweet. For for_you-origin tweets
    /// `rowid` is monotonic with browser-arrival order via
    /// `hydrate_for_you`'s queue-order traversal — that's what
    /// `bucket_sort` uses to preserve the operator's click order.
    rowid: i64,
    /// True iff at least one accepted origin for this tweet is
    /// `Origin::ForYou`. Determines which sort rule applies in
    /// `bucket_sort`: arrival order for for_you, `sort_by` for
    /// query-only.
    for_you: bool,
}

fn filter_with_priority(
    psyop: &PsyOp,
    entries: Vec<(Tweet, Vec<Origin>, i64)>,
) -> Result<Vec<Accepted>, Error> {
    let mut out = Vec::new();
    for (tweet, origins, rowid) in entries {
        let mut accepted_some_priority: Vec<Option<u64>> = Vec::new();
        let mut accepted_for_you = false;
        for origin in &origins {
            let (filter, priority) = match origin_lookup(psyop, origin) {
                Some(p) => p,
                None => continue, // origin no longer present in psyop config
            };
            let passes = match filter {
                Some(f) => crate::psyops::filter::evaluate(f, &tweet).map_err(Error::Other)?,
                None => true,
            };
            if passes {
                accepted_some_priority.push(priority);
                if matches!(origin, Origin::ForYou) {
                    accepted_for_you = true;
                }
            }
        }
        if accepted_some_priority.is_empty() {
            continue;
        }
        // Effective priority = smallest Some across all accepting
        // origins; None only if every accepting origin had no priority.
        let mut effective: Option<u64> = None;
        for p in accepted_some_priority {
            if let Some(p) = p {
                effective = Some(match effective {
                    None => p,
                    Some(curr) => curr.min(p),
                });
            }
        }
        out.push(Accepted { tweet, priority: effective, rowid, for_you: accepted_for_you });
    }
    Ok(out)
}

fn origin_lookup<'a>(
    psyop: &'a PsyOp,
    origin: &Origin,
) -> Option<(Option<&'a crate::psyops::Filter>, Option<u64>)> {
    match origin {
        Origin::ForYou => {
            // None propagates out — a stale `Origin::ForYou` row
            // from before `for_you` was removed gets dropped from
            // the accepted set, same as an unknown query name.
            let f = psyop.for_you.as_ref()?;
            Some((f.filter.as_ref(), f.priority))
        }
        Origin::Query(q) => {
            let qs = psyop.queries.as_ref()?;
            let matched: &Query = qs.iter().find(|qq| qq.query == *q)?;
            Some((matched.filter.as_ref(), matched.priority))
        }
    }
}

// -- step 5: bucket sort ---------------------------------------------------

fn bucket_sort(psyop: &PsyOp, accepted: Vec<Accepted>) -> Result<Vec<Tweet>, Error> {
    let mut buckets: BTreeMap<u64, Vec<Accepted>> = BTreeMap::new();
    let mut none_bucket: Vec<Accepted> = Vec::new();
    for a in accepted {
        match a.priority {
            Some(p) => buckets.entry(p).or_default().push(a),
            None    => none_bucket.push(a),
        }
    }
    let mut final_list = Vec::new();
    for (_p, bucket) in buckets {
        final_list.extend(sort_bucket(psyop, bucket)?);
    }
    final_list.extend(sort_bucket(psyop, none_bucket)?);
    Ok(final_list)
}

/// Within one priority bucket, for_you-origin tweets sort by
/// browser-arrival order (rowid ASC) and come before
/// query-origin tweets which sort by the psyop's `sort_by`.
/// Mixed-origin tweets (both query AND for_you sources accepted)
/// count as for_you — the operator's explicit pick outranks a
/// query match.
fn sort_bucket(psyop: &PsyOp, bucket: Vec<Accepted>) -> Result<Vec<Tweet>, Error> {
    let (mut fy, qs): (Vec<Accepted>, Vec<Accepted>) =
        bucket.into_iter().partition(|a| a.for_you);
    fy.sort_by_key(|a| a.rowid);
    let fy_tweets: Vec<Tweet> = fy.into_iter().map(|a| a.tweet).collect();
    let q_tweets:  Vec<Tweet> = qs.into_iter().map(|a| a.tweet).collect();
    let q_sorted = crate::psyops::sort_by::evaluate(&psyop.sort, q_tweets)
        .map_err(Error::Other)?;
    let mut out = fy_tweets;
    out.extend(q_sorted);
    Ok(out)
}

// -- step 4 helper: run queries -------------------------------------------

async fn run_queries(
    psyop: &PsyOp,
    db: &Db,
    http: &Client,
    auth: &AuthMode,
    name: &str,
) -> Result<(), Error> {
    let queries = match &psyop.queries {
        Some(qs) if !qs.is_empty() => qs,
        _ => return Ok(()),
    };
    for q in queries {
        if !matches!(q.endpoint, SearchEndpoint::Recent) {
            // `/2/tweets/search/all` is Pro/Enterprise only and not wired up
            // yet — skip with a notice.
            crate::output::OutputResult::from(crate::events::Event::QuerySkipped {
                psyop: name.to_string(),
                query: q.query.clone(),
                reason: "endpoint_not_recent".to_string(),
            })
            .emit();
            continue;
        }
        match search_recent(http, auth, &q.query).await {
            Ok(posts) => {
                crate::output::OutputResult::from(crate::events::Event::QueryComplete {
                    psyop: name.to_string(),
                    query: q.query.clone(),
                    count: posts.len(),
                })
                .emit();
                for p in posts {
                    db.insert_post(&p, name, &Origin::Query(q.query.clone())).await?;
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

// -- X API --------------------------------------------------------------------

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
    let resp = call(http, auth, &req).await.map_err(|e| {
        Error::Other(format!("X /2/tweets/{id} failed: {e}"))
    })?;
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
    let resp = call(http, auth, &req).await.map_err(|e| {
        Error::Other(format!("X /2/tweets/search/recent failed: {e}"))
    })?;
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
    let created = t
        .created_at
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();
    let (likes, retweets, replies, impressions) = match &t.public_metrics {
        Some(m) => (
            m.like_count    as u64,
            m.retweet_count as u64,
            m.reply_count   as u64,
            m.impression_count as u64,
        ),
        None => (0, 0, 0, 0),
    };
    let text = t.text.as_ref().map(|tt| tt.0.clone()).unwrap_or_default();
    Post {
        id,
        handle,
        text,
        images: Vec::new(),  // media expansion is a follow-up commit
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

// -- glue ---------------------------------------------------------------------

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

fn default_recent_request() -> psychological_operations_sdk::x::tweets::search::recent::get::Request {
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
