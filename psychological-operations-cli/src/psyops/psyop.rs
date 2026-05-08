use serde::{Deserialize, Serialize};

use super::for_you::ForYou;
use super::query::Query;
use super::sort_by::SortBy;
use super::stage::Stage;

/// A psyop scores tweets pulled from one or more X v2 sources.
#[derive(Debug, Serialize, Deserialize)]
pub struct PsyOp {
    /// Live X v2 search-query inputs. `None` means no query-driven
    /// ingestion for this psyop. An empty `Some(vec![])` is equivalent
    /// to `None` for ingestion purposes; both round-trip out as
    /// "field absent" via `skip_queries`.
    #[serde(default, skip_serializing_if = "skip_queries")]
    pub queries: Option<Vec<Query>>,
    /// Personalized "For You" timeline input.
    pub for_you: ForYou,

    /// Minimum total deduped candidates required before the psyop will
    /// run scoring. If the union of `queries` + `for_you` falls below
    /// this, the psyop is skipped.
    pub min_posts: u64,
    /// Hard cap on candidates sent to the scoring function. After
    /// dedup, the candidate set is ordered by `(priority, sort)` and
    /// truncated to `max_posts`.
    pub max_posts: u64,

    /// Tiebreak ordering applied across the deduped candidate union.
    /// Combines with per-source `priority` (priority is primary,
    /// descending; `None` ranks below every `Some(_)`. `sort` is the
    /// tiebreak among equal-priority items).
    pub sort: SortBy,

    /// When `false`, queries are skipped on a run as long as the
    /// for-you input still has queued candidates — the rationale
    /// being that if the algorithmic feed is feeding us enough
    /// material, paying for X v2 search calls is wasteful. When
    /// `true`, queries always run regardless of for-you queue state.
    /// Defaults to `true` (no implicit skipping).
    #[serde(default = "default_true")]
    pub query_when_for_you_queued: bool,

    /// Multi-stage scoring pipeline. Posts are scored by `stages[0]`,
    /// optionally narrowed via the stage's `output_threshold` /
    /// `output_top`, then fed to `stages[1]`, and so on. Must be
    /// non-empty (validated at publish time).
    pub stages: Vec<Stage>,
}

fn default_true() -> bool { true }

/// Skip-serializing predicate for `queries`: omit the field when
/// it's `None` OR `Some(empty)`. Both shapes mean "no query-driven
/// ingestion" so emitting `"queries": []` would just be noise.
fn skip_queries(q: &Option<Vec<Query>>) -> bool {
    match q {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// Read a psyop's JSON definition.
///
/// `commit_sha = None` → working-tree read of
/// `<psyops_dir>/<name>/psyop.json`.
/// `commit_sha = Some(sha)` → walk the named commit's tree in
/// `<psyops_dir>/<name>/`'s git repo and parse `psyop.json`'s blob.
/// Doesn't touch the working tree.
pub fn load(name: &str, commit_sha: Option<&str>) -> Result<PsyOp, crate::error::Error> {
    use crate::error::Error;
    let dir = crate::config::psyops_dir().join(name);

    let bytes: Vec<u8> = match commit_sha {
        None => {
            let path = dir.join("psyop.json");
            if !path.exists() {
                return Err(Error::PsyopNotFound(path.display().to_string()));
            }
            std::fs::read(&path)?
        }
        Some(sha) => {
            let repo = git2::Repository::open(&dir).map_err(|e| {
                Error::Other(format!("git open failed at {}: {e}", dir.display()))
            })?;
            let oid = git2::Oid::from_str(sha).map_err(|e| {
                Error::Other(format!("invalid commit sha \"{sha}\": {e}"))
            })?;
            let commit = repo.find_commit(oid).map_err(|e| {
                Error::Other(format!("commit {sha} not found in {}: {e}", dir.display()))
            })?;
            let tree = commit.tree().map_err(|e| {
                Error::Other(format!("commit {sha} tree lookup failed: {e}"))
            })?;
            let entry = tree.get_path(std::path::Path::new("psyop.json"))
                .map_err(|_| Error::PsyopNotFound(format!(
                    "{}@{sha}:psyop.json", dir.display(),
                )))?;
            let object = entry.to_object(&repo).map_err(|e| {
                Error::Other(format!("psyop.json blob lookup failed at {sha}: {e}"))
            })?;
            let blob = object.as_blob().ok_or_else(|| {
                Error::Other(format!("psyop.json at {sha} is not a blob"))
            })?;
            blob.content().to_vec()
        }
    };

    Ok(serde_json::from_slice(&bytes)?)
}

/// Write a psyop's JSON definition back to disk (pretty-printed).
pub fn save(name: &str, psyop: &PsyOp) -> Result<(), crate::error::Error> {
    let path = crate::config::psyops_dir().join(name).join("psyop.json");
    let json = serde_json::to_string_pretty(psyop)?;
    std::fs::write(&path, json + "\n")?;
    Ok(())
}

impl PsyOp {
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        let bad = |s: String| crate::error::Error::InvalidPsyop(s);

        if self.max_posts == 0 {
            return Err(bad("max_posts must be > 0".into()));
        }
        if self.min_posts > self.max_posts {
            return Err(bad("min_posts must be <= max_posts".into()));
        }

        if let Some(qs) = &self.queries {
            for (i, q) in qs.iter().enumerate() {
                q.validate().map_err(|e| bad(format!("queries[{i}]: {e}")))?;
            }
        }
        self.for_you.validate().map_err(|e| bad(format!("for_you: {e}")))?;
        self.sort.validate().map_err(|e| bad(format!("sort: {e}")))?;

        if self.stages.is_empty() {
            return Err(bad("stages must not be empty".into()));
        }
        for (i, s) in self.stages.iter().enumerate() {
            s.validate().map_err(|e| bad(format!("stages[{i}]: {e}")))?;
        }

        Ok(())
    }
}
