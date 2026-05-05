use serde::{Deserialize, Serialize};
use objectiveai::functions::{
    FullInlineFunctionOrRemoteCommitOptional,
    FullInlineFunction,
    AlphaInlineFunction,
    InlineFunction,
    InlineProfileOrRemoteCommitOptional,
};
use objectiveai::functions::executions::request::Strategy;

/// Per-tweet eligibility filter. Applied AFTER fetch (or after DB
/// selection for `Source::Scored`), BEFORE the scoring function runs.
/// Shared across `Source` variants.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Filter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_likes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_retweets: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_replies: Option<u64>,
    /// Reject tweets whose `created` is older than this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u64>,
    /// Reject tweets whose `created` is younger than this many seconds.
    /// Useful for letting engagement settle before scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_age: Option<u64>,
}

/// Which X v2 search endpoint a `Source::Search` should hit.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchEndpoint {
    /// `/2/tweets/search/recent` — last 7 days, all access tiers.
    Recent,
    /// `/2/tweets/search/all` — full archive (Pro / Enterprise tiers).
    All,
}

impl Default for SearchEndpoint {
    fn default() -> Self { Self::Recent }
}

/// Where a psyop's candidate tweets come from. A psyop may declare
/// multiple sources; their results are deduped by tweet id and unioned
/// before scoring.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Live X v2 search query. Hits the chosen `endpoint` with `query`,
    /// paginates up to `count` tweets, persists them to the local DB
    /// alongside the psyop's `tags`, then applies `filter` for
    /// score-time eligibility.
    Search {
        /// X v2 search-operator string (e.g. `"from:user has:media -is:retweet"`).
        query: String,
        #[serde(default)]
        endpoint: SearchEndpoint,
        /// Cap on tweets pulled from this source (drives pagination).
        /// `None` means "fetch as many as the API will return for this
        /// query in one window".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<u64>,
        #[serde(default)]
        filter: Filter,
    },
    /// Cascade input: pull previously-scored posts in the local DB
    /// carrying `tag` (an output tag of an upstream psyop), optionally
    /// gated by `min_score`. Enables psyop chains where one psyop's
    /// scores filter another's input.
    Scored {
        tag: String,
        /// Only consider posts whose stored score under any psyop is
        /// `>= min_score`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_score: Option<f64>,
        /// Cap on how many posts to draw from this source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<u64>,
        #[serde(default)]
        filter: Filter,
    },
}

impl Source {
    pub fn count(&self) -> Option<u64> {
        match self {
            Source::Search { count, .. } => *count,
            Source::Scored { count, .. } => *count,
        }
    }

    pub fn filter(&self) -> &Filter {
        match self {
            Source::Search { filter, .. } => filter,
            Source::Scored { filter, .. } => filter,
        }
    }
}

/// A psyop scores tweets pulled from one or more `Source`s. Output
/// scores get persisted with `tags` so downstream psyops can cascade
/// off them via `Source::Scored`.
#[derive(Debug, Serialize, Deserialize)]
pub struct PsyOp {
    pub sources: Vec<Source>,
    /// Tags applied to every score row this psyop produces. Other psyops
    /// can then select these scores via `Source::Scored.tag`. Must
    /// contain at least one tag.
    pub tags: Vec<String>,
    pub function: FullInlineFunctionOrRemoteCommitOptional,
    pub profile: InlineProfileOrRemoteCommitOptional,
    pub strategy: Strategy,
    #[serde(default)]
    pub invert: bool,
    /// If `false`, scored posts are sent to the function with an empty
    /// `images` array regardless of what was ingested. Defaults to `true`.
    #[serde(default = "default_true")]
    pub images: bool,
    /// If `false`, scored posts are sent to the function with an empty
    /// `videos` array regardless of what was ingested. Defaults to `true`.
    #[serde(default = "default_true")]
    pub videos: bool,
    /// Minimum total candidates (across all sources, deduped) required to
    /// run. The effective floor is `max(2, count.unwrap_or(0))`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

fn default_true() -> bool { true }

/// Read a psyop's JSON definition from disk.
pub fn load(name: &str) -> Result<PsyOp, crate::error::Error> {
    let path = crate::config::psyops_dir().join(name).join("psyop.json");
    if !path.exists() {
        return Err(crate::error::Error::PsyopNotFound(path.display().to_string()));
    }
    let data = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&data)?)
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
        if self.sources.is_empty() {
            return Err(crate::error::Error::InvalidPsyop("sources must not be empty".into()));
        }
        if self.tags.is_empty() {
            return Err(crate::error::Error::InvalidPsyop("tags must not be empty".into()));
        }
        for (i, src) in self.sources.iter().enumerate() {
            if let Source::Search { query, .. } = src {
                if query.trim().is_empty() {
                    return Err(crate::error::Error::InvalidPsyop(
                        format!("sources[{i}]: search query must not be empty"),
                    ));
                }
            }
            if let Source::Scored { tag, .. } = src {
                if tag.trim().is_empty() {
                    return Err(crate::error::Error::InvalidPsyop(
                        format!("sources[{i}]: scored tag must not be empty"),
                    ));
                }
            }
        }
        Ok(())
    }
}

pub struct ValidationResult {
    pub valid: bool,
    pub reason: Option<&'static str>,
}

/// Per-tweet score-time eligibility check against a `Filter`.
pub fn valid_for_filter(
    filter: &Filter,
    created: &str,
    likes: u64,
    retweets: u64,
    replies: u64,
    now: &chrono::DateTime<chrono::Utc>,
) -> ValidationResult {
    if let Ok(created_time) = chrono::DateTime::parse_from_rfc3339(created) {
        let age_seconds = (*now - created_time.with_timezone(&chrono::Utc)).num_seconds();
        if let Some(max_age) = filter.max_age {
            if age_seconds > max_age as i64 {
                return ValidationResult { valid: false, reason: Some("max_age") };
            }
        }
        if let Some(min_age) = filter.min_age {
            if age_seconds < min_age as i64 {
                return ValidationResult { valid: false, reason: Some("min_age") };
            }
        }
    }
    if let Some(min_likes) = filter.min_likes {
        if likes < min_likes {
            return ValidationResult { valid: false, reason: Some("min_likes") };
        }
    }
    if let Some(min_retweets) = filter.min_retweets {
        if retweets < min_retweets {
            return ValidationResult { valid: false, reason: Some("min_retweets") };
        }
    }
    if let Some(min_replies) = filter.min_replies {
        if replies < min_replies {
            return ValidationResult { valid: false, reason: Some("min_replies") };
        }
    }
    ValidationResult { valid: true, reason: None }
}

/// Determine if a function is a vector function.
/// If the function is remote, it must be fetched first (caller resolves it).
pub fn is_vector_function(function: &FullInlineFunction) -> bool {
    match function {
        FullInlineFunction::Alpha(alpha) => matches!(alpha, AlphaInlineFunction::Vector(_)),
        FullInlineFunction::Standard(standard) => matches!(standard, InlineFunction::Vector { .. }),
    }
}
