//! `psyops run` — run every enabled psyop in rounds. Each round picks the
//! psyops that currently have enough data, runs them concurrently, persists
//! their scores, and then re-evaluates eligibility. The loop terminates
//! once a round finds no eligible psyops, so cascading psyops (whose
//! `Source.min_score` depends on another psyop's just-written scores)
//! get a chance to run after their upstream completes.

use std::collections::HashSet;

use crate::db::UnscoredEntry;
use super::PsyOp;

pub async fn run_all(name_filter: Option<&str>, commit_filter: Option<&str>) -> Result<crate::Output, crate::error::Error> {
    let cfg = crate::config::load();
    let dir = crate::config::psyops_dir();
    if !dir.exists() {
        eprintln!("no psyops directory at {}", dir.display());
        return Ok(crate::Output::Empty);
    }

    let mut targets: Vec<String> = Vec::new();
    for ent in std::fs::read_dir(&dir)? {
        let ent = ent?;
        let path = ent.path();
        if !path.is_dir()
            || !path.join("psyop.json").exists()
            || !path.join(".git").exists()
        {
            continue;
        }
        let Some(name) = ent.file_name().to_str().map(|s| s.to_string()) else { continue };

        if let Some(want) = name_filter {
            if name != want { continue; }
        }

        let commit_sha = (|| -> Result<String, git2::Error> {
            let repo = git2::Repository::open(&path)?;
            let head = repo.head()?.peel_to_commit()?;
            Ok(head.id().to_string())
        })().unwrap_or_default();

        if let Some(want_commit) = commit_filter {
            if commit_sha != want_commit {
                eprintln!(
                    "psyop \"{name}\" HEAD is {commit_sha}, not requested commit {want_commit}; skipping",
                );
                continue;
            }
        }

        // `--name` is an explicit operator request; override the disabled
        // gate so a "disable then run standalone" workflow is possible.
        if name_filter.is_none()
            && cfg.psyops.get(&name).is_some_and(|o| o.disabled_for(&commit_sha))
        {
            eprintln!("psyop \"{name}\" is disabled for commit {commit_sha}; skipping");
            continue;
        }

        targets.push(name);
    }

    if targets.is_empty() {
        eprintln!("no enabled psyops to run");
        return Ok(crate::Output::Empty);
    }

    // Names that errored out at any point this run (eligibility check,
    // task panic, or run_psyop error). Once a psyop is in here it's
    // permanently excluded from later rounds — re-running the same psyop
    // round-after-round was producing the same error each time, wasting
    // upstream calls. The set also drives the final exit code.
    let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut round: u32 = 0;
    loop {
        round += 1;
        let mut eligible: Vec<String> = Vec::new();
        for name in &targets {
            if failed.contains(name) { continue; }
            match has_enough_data(name) {
                Ok(true) => eligible.push(name.clone()),
                Ok(false) => {}
                Err(e) => {
                    eprintln!("psyop \"{name}\" eligibility check failed: {e}");
                    failed.insert(name.clone());
                }
            }
        }
        if eligible.is_empty() {
            if round == 1 {
                eprintln!("no psyops have enough data to run");
            } else {
                eprintln!("round {round}: nothing eligible; stopping");
            }
            break;
        }
        eprintln!("round {round}: running {} psyop(s) concurrently", eligible.len());

        let handles: Vec<_> = eligible.into_iter().map(|name| {
            tokio::spawn(async move {
                let result = run_psyop(&name).await;
                (name, result)
            })
        }).collect();

        for join in futures::future::join_all(handles).await {
            let (name, result) = join.unwrap();
            match result {
                Ok(_) => eprintln!("psyop \"{name}\" finished"),
                Err(e) => {
                    eprintln!("psyop \"{name}\" failed: {e}");
                    failed.insert(name);
                }
            }
        }
    }

    if !failed.is_empty() {
        let mut sorted: Vec<&String> = failed.iter().collect();
        sorted.sort();
        return Err(crate::error::Error::Other(format!(
            "{} psyop(s) failed: {}",
            failed.len(),
            sorted.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        )));
    }

    Ok(crate::Output::Empty)
}

/// Eligibility rules:
///   - Every `Source` with `Some(count)` must have at least `count`
///     eligible candidates pulled from *that* source.
///   - The deduped union across sources must reach `max(2, psyop.count)`.
fn has_enough_data(name: &str) -> Result<bool, crate::error::Error> {
    let psyop_dir = crate::config::psyops_dir().join(name);
    let psyop_path = psyop_dir.join("psyop.json");
    if !psyop_path.exists() { return Ok(false); }

    let data = std::fs::read_to_string(&psyop_path)?;
    let psyop: PsyOp = serde_json::from_str(&data)?;
    psyop.validate()?;

    let commit_sha = {
        let repo = git2::Repository::open(&psyop_dir)?;
        let head = repo.head()?.peel_to_commit()?;
        head.id().to_string()
    };

    let now = chrono::Utc::now();
    let db = crate::db::Db::open()?;
    let mut union_size: u64 = 0;
    let mut seen: HashSet<String> = HashSet::new();
    for source in &psyop.sources {
        let pool = collect_for_source(&db, name, &commit_sha, source, &now)?;
        if let Some(target) = source.count {
            if (pool.len() as u64) < target {
                return Ok(false);
            }
        }
        for entry in pool {
            if seen.insert(entry.post.id.clone()) {
                union_size += 1;
            }
        }
    }
    Ok(union_size >= psyop.count.unwrap_or(0).max(2))
}

/// Run a single psyop end-to-end. Score the union of per-source candidates
/// via the psyop's function, persist the scores, fire notifications.
async fn run_psyop(name: &str) -> Result<(), crate::error::Error> {
    let cfg = crate::config::load();
    let psyop_dir = crate::config::psyops_dir().join(name);
    let psyop_path = psyop_dir.join("psyop.json");
    if !psyop_path.exists() {
        return Err(crate::error::Error::PsyopNotFound(psyop_path.display().to_string()));
    }

    let data = std::fs::read_to_string(&psyop_path)?;
    let psyop: PsyOp = serde_json::from_str(&data)?;
    psyop.validate()?;

    // Scope libgit2 handles tightly: they're !Send, so any await afterwards
    // would poison the tokio::spawn future if we held them open.
    let commit_sha = {
        let repo = git2::Repository::open(&psyop_dir)?;
        let head = repo.head()?.peel_to_commit()?;
        head.id().to_string()
    };

    let now = chrono::Utc::now();
    let db = crate::db::Db::open()?;

    let mut entries: Vec<UnscoredEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for source in &psyop.sources {
        let pool = collect_for_source(&db, name, &commit_sha, source, &now)?;
        for entry in pool {
            if seen.insert(entry.post.id.clone()) {
                entries.push(entry);
            }
        }
    }

    let mut scored_posts: Vec<crate::score::ScoredPost> = Vec::new();
    if !entries.is_empty() {
        scored_posts = crate::score::score(&psyop, entries)?;
        let ids: Vec<String> = scored_posts.iter().map(|s| s.post.id.clone()).collect();
        let scores: Vec<f64> = scored_posts.iter().map(|s| s.score).collect();
        db.set_scores(name, &commit_sha, &ids, &scores, &psyop.tags)?;
    }

    let output: Vec<&crate::score::ScoredPost> = scored_posts.iter().collect();

    let mut destinations = cfg.notifications.clone();
    if let Some(per_psyop) = cfg.psyops.get(name) {
        destinations.extend(per_psyop.notifications_for(&commit_sha).iter().cloned());
    }
    crate::notifications::destinations::notify(
        &destinations,
        crate::notifications::destinations::Subject::Psyop {
            name,
            psyop: &psyop,
            output: &output,
        },
    ).await;

    Ok(())
}

/// Pull `source.count` oldest unscored posts carrying `source.tag` for
/// `(psyop, commit)`, then apply per-tweet eligibility (`valid_for_source`)
/// and an over-fetch retry loop until the source's count is satisfied or
/// the pool runs dry.
fn collect_for_source(
    db: &crate::db::Db,
    psyop_name: &str,
    commit_sha: &str,
    source: &Source,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<Vec<UnscoredEntry>, crate::error::Error> {
    let target = source.count.map(|n| n as usize);
    let tags = vec![source.tag.clone()];
    // Pull a generous over-fetch so per-tweet validation has room to
    // discard ineligible candidates without going back to the DB.
    let pull_limit = target.map(|t| t.saturating_mul(4).max(t + 16)).unwrap_or(usize::MAX / 4);
    let raw = db.get_oldest_unscored_for_tags(psyop_name, commit_sha, &tags, source.min_score, pull_limit)?;

    let mut accepted: Vec<UnscoredEntry> = Vec::new();
    for entry in raw {
        let v = valid_for_source(
            source,
            &entry.post.created,
            entry.post.likes,
            entry.post.retweets,
            entry.post.replies,
            now,
        );
        if v.valid {
            accepted.push(entry);
            if let Some(t) = target {
                if accepted.len() >= t { break; }
            }
        }
    }
    Ok(accepted)
}
