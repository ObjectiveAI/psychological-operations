//! `psyops run` — execute a single psyop end-to-end.
//!
//! Per-psyop flow:
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

use crate::db::{Db, Origin, Post};
use crate::error::Error;
use crate::score::{self, ScoredPost};
use crate::tweet::Tweet;
use psychological_operations_sdk::x::client::Client;
use psychological_operations_sdk::x::params::tweet_expansions_parameter::TweetExpansions;
use psychological_operations_sdk::x::params::tweet_fields_parameter::TweetFields;
use psychological_operations_sdk::x::params::user_fields_parameter::UserFields;
use psychological_operations_sdk::x::types::TweetId;

use super::query::SearchEndpoint;
use super::{ForYou, PsyOp, Query};

/// CLI entrypoint kept for `psyops::Commands::Run` — name + optional
/// explicit commit override. The `commit_filter` is honored as a
/// hard override on the on-disk psyop's HEAD commit.
pub async fn run_all(
    name_filter: Option<&str>,
    commit_filter: Option<&str>,
    seed: Option<i64>,
    cfg: &crate::run::Config,
) -> Result<crate::Output, Error> {
    let name = name_filter.ok_or_else(|| {
        Error::Other("psyops run requires --name <psyop>".into())
    })?;
    run_psyop(name, commit_filter, seed, cfg).await
}

pub async fn run_psyop(
    name: &str,
    commit_override: Option<&str>,
    seed: Option<i64>,
    cfg: &crate::run::Config,
) -> Result<crate::Output, Error> {
    let psyop = super::psyop::load(name, None, cfg)?;
    psyop.validate()?;

    // Gate the X-app credential preflight on whether this psyop is
    // mocked. Mocked psyops never touch the real X API, so requiring
    // `x_app setup` for them would be pointless friction.
    if !psyop.mock_enabled() {
        psychological_operations_sdk::x::x_app::config::ensure_setup(&cfg.objectiveai_base_dir())
            .map_err(|e| Error::Other(format!("x_app.json: {e}")))?;
    }

    let commit = match commit_override {
        Some(c) => c.to_string(),
        None => derive_commit(name, cfg)?,
    };

    let db = Db::open(cfg)?;
    let http = make_http_client(psyop.mock_enabled(), cfg).await?;

    // Capture whether the for_you_queue was non-empty at run start —
    // the `query_when_for_you_queued` policy reads this on the
    // re-loop iteration to decide whether queries are allowed.
    let queue_at_start = db.for_you_queue(name, &commit)?;
    let had_for_you_queued_at_start = !queue_at_start.is_empty();
    let mut queries_already_ran = false;

    loop {
        // 1. Hydrate the for-you queue (drains everything currently in it).
        hydrate_for_you(&db, &http, name, &commit).await?;

        // 2. Read unscored tweets for this (psyop, commit).
        let now = chrono::Utc::now();
        let entries = db.list_unscored_with_origins(name, &commit, &now)?;

        // 3. Filter with priority resolution.
        let accepted = filter_with_priority(&psyop, entries)?;

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
            run_queries(&psyop, &db, &http, name, &commit).await?;
            queries_already_ran = true;
            continue;
        }

        // 5. Priority-bucket sort.
        let final_list = bucket_sort(&psyop, accepted)?;

        // 6. Trim to max_posts.
        let trimmed: Vec<Tweet> = final_list
            .into_iter()
            .take(psyop.max_posts as usize)
            .collect();

        // 7. Hydrate Tweet -> Post by joining with the `contents`
        //    table, then run the multi-stage scoring pipeline.
        let result = score_pipeline(&db, &psyop, name, trimmed, seed, cfg)?;

        // 8. Persist scores for every scored post.
        if !result.last_scores.is_empty() {
            let ids: Vec<String> = result.last_scores.keys().cloned().collect();
            let scores: Vec<f64> = ids.iter().map(|id| result.last_scores[id]).collect();
            db.set_scores(&ids, &scores)?;
        }

        // 9. Reap content for every post under (name, commit), scored
        //    or not.
        let _dropped = db.drop_psyop_contents(name, &commit)?;

        // 10. Enqueue a delivery_queue row per (target, survivors).
        if !result.survivors.is_empty() {
            let json_cfg = crate::config::load(cfg);
            let post_ids: Vec<String> = result.survivors.iter()
                .map(|s| s.post.id.clone())
                .collect();
            let post_ids_json = serde_json::to_string(&post_ids)?;

            for dest in &json_cfg.targets {
                let target_json = serde_json::to_string(dest)?;
                db.enqueue_delivery(name, &commit, &target_json, &post_ids_json)?;
            }
            let per_psyop: Vec<crate::targets::destinations::Destination> =
                json_cfg.psyops.get(name)
                    .map(|o| o.targets_for(&commit).to_vec())
                    .unwrap_or_default();
            for dest in &per_psyop {
                let target_json = serde_json::to_string(dest)?;
                db.enqueue_delivery(name, &commit, &target_json, &post_ids_json)?;
            }
        }

        // 11. Drain the queue (filtered to this psyop).
        let _summary = crate::targets::drain_queue(&db, Some(name), cfg).await?;

        return Ok(crate::Output::Empty);
    }
}

/// Output of `score_pipeline` — every post that got a score, plus the
/// final survivors of all stages (which are what targets fire against).
struct ScoreResult {
    last_scores: HashMap<String, f64>,
    survivors:   Vec<ScoredPost>,
}

// -- step 7: score pipeline -----------------------------------------------

fn score_pipeline(
    db: &Db,
    psyop: &PsyOp,
    name: &str,
    trimmed: Vec<Tweet>,
    seed: Option<i64>,
    cfg: &crate::run::Config,
) -> Result<ScoreResult, Error> {
    // Hydrate Tweet -> Post via contents lookup. Tweets whose
    // contents row is absent are filtered out — by contract those
    // posts don't exist for our purposes.
    let ids: Vec<String> = trimmed.iter().map(|t| t.id.clone()).collect();
    let contents = db.fetch_contents(&ids)?;
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

    // Each post's score = the LAST stage that scored it. Survivors
    // of every stage end up with the final stage's score; posts
    // dropped at stage K end up with stage K's score.
    let mut last_scores: HashMap<String, f64> = HashMap::new();
    let mut survivors: Vec<ScoredPost> = Vec::new();

    for (i, stage) in psyop.stages.iter().enumerate() {
        if current.is_empty() {
            crate::emit::emit(crate::events::Event::StageEmpty {
                psyop: name.to_string(),
                stage: i,
            });
            break;
        }

        // Bracket each stage with marker notifications so consumers
        // can see exactly where one stage ends and the next begins in
        // the JSONL stream. Snapshot wire shape (after host re-wrap):
        //   {"type":"notification","value":{"event":"stage_begin","stage":N}}
        //   …per-stage scoring notifications…
        //   {"type":"notification","value":{"event":"stage_end","stage":N}}
        crate::emit::emit(crate::events::Event::StageBegin { stage: i });

        let scored: Vec<ScoredPost> = score::score(stage, current, seed, cfg)?;
        for s in &scored {
            last_scores.insert(s.post.id.clone(), s.score);
        }

        // output_threshold: drop scores < threshold.
        let after_threshold: Vec<ScoredPost> = match stage.output_threshold {
            Some(t) => scored.into_iter().filter(|s| s.score >= t).collect(),
            None => scored,
        };

        // output_top: keep top ceil(N * pct).
        let after_top: Vec<ScoredPost> = match stage.output_top {
            Some(p) if !after_threshold.is_empty() => {
                let n = ((after_threshold.len() as f64) * p).ceil() as usize;
                after_threshold.into_iter().take(n).collect()
            }
            _ => after_threshold,
        };

        survivors = after_top.clone();
        current = after_top.into_iter().map(|s| s.post).collect();

        crate::emit::emit(crate::events::Event::StageEnd { stage: i });
    }

    Ok(ScoreResult { last_scores, survivors })
}

// -- step 1: hydrate -------------------------------------------------------

async fn hydrate_for_you(
    db: &Db,
    http: &Client,
    name: &str,
    commit: &str,
) -> Result<(), Error> {
    let queued = db.for_you_queue(name, commit)?;
    if queued.is_empty() {
        return Ok(());
    }
    crate::emit::emit(crate::events::Event::HydratingQueue {
        psyop: name.to_string(),
        count: queued.len(),
    });
    let mut succeeded: Vec<String> = Vec::new();
    for id in queued {
        match fetch_tweet(http, &id).await {
            Ok(Some(post)) => {
                db.insert_post(&post, name, commit, &Origin::ForYou)?;
                succeeded.push(id);
            }
            Ok(None) => {
                crate::emit::emit(crate::events::Event::TweetNotFound {
                    psyop: name.to_string(),
                    tweet_id: id.clone(),
                });
                succeeded.push(id);   // unrecoverable — don't keep retrying
            }
            Err(e) => {
                crate::emit::emit(crate::events::Event::TweetFetchFailed {
                    psyop: name.to_string(),
                    tweet_id: id,
                    error: e.to_string(),
                });
                // leave in queue for next round
            }
        }
    }
    db.dequeue_for_you(name, commit, &succeeded)?;
    Ok(())
}

// -- step 3: filter --------------------------------------------------------

struct Accepted {
    tweet: Tweet,
    /// Smallest `Some(_)` priority across this tweet's accepting
    /// origins; `None` if no accepting origin had a priority set.
    priority: Option<u64>,
}

fn filter_with_priority(
    psyop: &PsyOp,
    entries: Vec<(Tweet, Vec<Origin>)>,
) -> Result<Vec<Accepted>, Error> {
    let mut out = Vec::new();
    for (tweet, origins) in entries {
        let mut accepted_some_priority: Vec<Option<u64>> = Vec::new();
        for origin in &origins {
            let (filter, priority) = match origin_lookup(psyop, origin) {
                Some(p) => p,
                None => continue, // origin no longer present in psyop config
            };
            let passes = match filter {
                Some(f) => f.evaluate(&tweet).map_err(Error::Other)?,
                None => true,
            };
            if passes {
                accepted_some_priority.push(priority);
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
        out.push(Accepted { tweet, priority: effective });
    }
    Ok(out)
}

fn origin_lookup<'a>(
    psyop: &'a PsyOp,
    origin: &Origin,
) -> Option<(Option<&'a super::filter::Filter>, Option<u64>)> {
    match origin {
        Origin::ForYou => {
            let f: &ForYou = &psyop.for_you;
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
    let mut buckets: BTreeMap<u64, Vec<Tweet>> = BTreeMap::new();
    let mut none_bucket: Vec<Tweet> = Vec::new();
    for a in accepted {
        match a.priority {
            Some(p) => buckets.entry(p).or_default().push(a.tweet),
            None    => none_bucket.push(a.tweet),
        }
    }
    let mut final_list = Vec::new();
    for (_p, bucket) in buckets {
        final_list.extend(psyop.sort.evaluate(bucket).map_err(Error::Other)?);
    }
    final_list.extend(psyop.sort.evaluate(none_bucket).map_err(Error::Other)?);
    Ok(final_list)
}

// -- step 4 helper: run queries -------------------------------------------

async fn run_queries(
    psyop: &PsyOp,
    db: &Db,
    http: &Client,
    name: &str,
    commit: &str,
) -> Result<(), Error> {
    let queries = match &psyop.queries {
        Some(qs) if !qs.is_empty() => qs,
        _ => return Ok(()),
    };
    for q in queries {
        if !matches!(q.endpoint, SearchEndpoint::Recent) {
            // `/2/tweets/search/all` is Pro/Enterprise only and not wired up
            // yet — skip with a notice.
            crate::emit::emit(crate::events::Event::QuerySkipped {
                psyop: name.to_string(),
                query: q.query.clone(),
                reason: "endpoint_not_recent".to_string(),
            });
            continue;
        }
        match search_recent(http, &q.query).await {
            Ok(posts) => {
                crate::emit::emit(crate::events::Event::QueryComplete {
                    psyop: name.to_string(),
                    query: q.query.clone(),
                    count: posts.len(),
                });
                for p in posts {
                    db.insert_post(&p, name, commit, &Origin::Query(q.query.clone()))?;
                }
            }
            Err(e) => {
                crate::emit::emit(crate::events::Event::QueryFailed {
                    psyop: name.to_string(),
                    query: q.query.clone(),
                    error: e.to_string(),
                });
            }
        }
    }
    Ok(())
}

// -- X API --------------------------------------------------------------------

async fn make_http_client(mock: bool, cfg: &crate::run::Config) -> Result<Client, Error> {
    Client::app_only(
        reqwest::Client::new(),
        mock,
        &cfg.objectiveai_base_dir(),
        // Bytes — explicit per-call size budget for the SQLite response
        // cache. No `DEFAULT_*` constant — `Client::*` makes this a
        // required arg and every CLI callsite picks its own value.
        256 * 1024 * 1024,
        // Cache entry TTL — plumbed but unused today (future
        // time-based eviction will consume it).
        std::time::Duration::from_secs(3600),
    )
    .await
    .map_err(|e| Error::Other(format!("Client::app_only: {e}")))
}

fn standard_tweet_fields() -> Vec<TweetFields> {
    vec![
        TweetFields::CreatedAt,
        TweetFields::PublicMetrics,
        TweetFields::AuthorId,
    ]
}

async fn fetch_tweet(http: &Client, id: &str) -> Result<Option<Post>, Error> {
    use psychological_operations_sdk::x::tweets::id::get;
    use psychological_operations_sdk::x::tweets::id::http::get as call;
    let req = get::Request {
        id: TweetId(id.to_string()),
        tweet_fields: Some(standard_tweet_fields()),
        expansions: Some(vec![TweetExpansions::AuthorId]),
        user_fields: Some(vec![UserFields::Username]),
        ..default_id_request()
    };
    let resp = call(http, &req).await.map_err(|e| {
        Error::Other(format!("X /2/tweets/{id} failed: {e}"))
    })?;
    let tweet = match resp.data {
        Some(t) => t,
        None => return Ok(None),
    };
    Ok(Some(tweet_to_post(&tweet, resp.includes.as_ref())))
}

async fn search_recent(http: &Client, query: &str) -> Result<Vec<Post>, Error> {
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
    let resp = call(http, &req).await.map_err(|e| {
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

fn derive_commit(name: &str, cfg: &crate::run::Config) -> Result<String, Error> {
    let dir = crate::config::psyops_dir(cfg).join(name);
    let repo = git2::Repository::open(&dir).map_err(|e| {
        Error::Other(format!("git open failed at {}: {e}", dir.display()))
    })?;
    let head = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
        Error::Other(format!("git HEAD lookup failed: {e}"))
    })?;
    Ok(head.id().to_string())
}

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
