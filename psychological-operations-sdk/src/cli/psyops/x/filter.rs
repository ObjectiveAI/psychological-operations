use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Per-tweet eligibility filter. Shared by `Query` and `ForYou` —
/// both attach an `Option<Filter>` so a source with no filter accepts
/// every tweet that the source itself produces.
///
/// Field ordering alternates `min_X` / `max_X` for each engagement
/// metric, then closes with `min_age` / `max_age`. The age fields
/// gate by `created` distance from now (in seconds): `min_age` lets
/// engagement settle before scoring, `max_age` rejects tweets older
/// than the cutoff.
///
/// `python` is an optional Python boolean expression that
/// AND-combines with the static gates above. Runtime evaluation
/// lives in the CLI (it runs the code via the `python` command
/// against `&Tweet`); only publish-time `validate` lives here.
#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
pub struct Filter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_likes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_likes: Option<u64>,
    /// Floor on `likes / impressions` (range `0.0..=1.0`). When
    /// `impressions == 0` the observed ratio is treated as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_likes_per_impression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_likes_per_impression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_retweets: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retweets: Option<u64>,
    /// Floor on `retweets / impressions` (range `0.0..=1.0`). When
    /// `impressions == 0` the observed ratio is treated as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_retweets_per_impression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retweets_per_impression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_replies: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_replies: Option<u64>,
    /// Floor on `replies / impressions` (range `0.0..=1.0`). When
    /// `impressions == 0` the observed ratio is treated as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_replies_per_impression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_replies_per_impression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_impressions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_impressions: Option<u64>,
    /// Reject tweets whose `created` is younger than this many seconds.
    /// Useful for letting engagement settle before scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_age: Option<u64>,
    /// Reject tweets whose `created` is older than this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u64>,
    /// Optional Python boolean expression. The `input` global is a
    /// dict with keys `likes`, `retweets`, `replies`, `impressions`,
    /// `age` (all `int`, age in seconds). Its trailing expression must
    /// evaluate to `bool` — non-bool results are rejected as errors,
    /// not coerced. AND-combines with the static gates above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
}

impl Filter {
    /// Validate the filter at publish time:
    ///   - For every `min_X` / `max_X` pair (counts and ratios),
    ///     both bounds must be consistent (`min <= max`) when both
    ///     are set.
    ///   - Every `_per_impression` ratio bound must lie in `[0, 1]`.
    /// (`python` is not parse-checked here — errors surface at run time.)
    pub fn validate(&self) -> Result<(), String> {
        check_pair("likes", self.min_likes, self.max_likes)?;
        check_pair("retweets", self.min_retweets, self.max_retweets)?;
        check_pair("replies", self.min_replies, self.max_replies)?;
        check_pair("impressions", self.min_impressions, self.max_impressions)?;
        check_pair("age", self.min_age, self.max_age)?;

        check_ratio("min_likes_per_impression", self.min_likes_per_impression)?;
        check_ratio("max_likes_per_impression", self.max_likes_per_impression)?;
        check_ratio(
            "min_retweets_per_impression",
            self.min_retweets_per_impression,
        )?;
        check_ratio(
            "max_retweets_per_impression",
            self.max_retweets_per_impression,
        )?;
        check_ratio(
            "min_replies_per_impression",
            self.min_replies_per_impression,
        )?;
        check_ratio(
            "max_replies_per_impression",
            self.max_replies_per_impression,
        )?;

        check_ratio_pair(
            "likes_per_impression",
            self.min_likes_per_impression,
            self.max_likes_per_impression,
        )?;
        check_ratio_pair(
            "retweets_per_impression",
            self.min_retweets_per_impression,
            self.max_retweets_per_impression,
        )?;
        check_ratio_pair(
            "replies_per_impression",
            self.min_replies_per_impression,
            self.max_replies_per_impression,
        )?;

        Ok(())
    }
}

fn check_pair(name: &str, min: Option<u64>, max: Option<u64>) -> Result<(), String> {
    if let (Some(lo), Some(hi)) = (min, max) {
        if lo > hi {
            return Err(format!("min_{name} ({lo}) must be <= max_{name} ({hi})",));
        }
    }
    Ok(())
}

fn check_ratio(name: &str, value: Option<f64>) -> Result<(), String> {
    if let Some(v) = value {
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            return Err(format!("{name} ({v}) must be in [0.0, 1.0]"));
        }
    }
    Ok(())
}

fn check_ratio_pair(name: &str, min: Option<f64>, max: Option<f64>) -> Result<(), String> {
    if let (Some(lo), Some(hi)) = (min, max) {
        if lo > hi {
            return Err(format!("min_{name} ({lo}) must be <= max_{name} ({hi})",));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_greater_than_max_rejected() {
        let f = Filter {
            min_likes: Some(100),
            max_likes: Some(50),
            ..Default::default()
        };
        let err = f.validate().unwrap_err();
        assert!(err.contains("min_likes"));
        assert!(err.contains("max_likes"));
    }

    #[test]
    fn equal_min_max_is_fine() {
        let f = Filter {
            min_likes: Some(50),
            max_likes: Some(50),
            ..Default::default()
        };
        f.validate().unwrap();
    }

    #[test]
    fn min_set_alone_is_fine() {
        let f = Filter {
            min_age: Some(60),
            ..Default::default()
        };
        f.validate().unwrap();
    }

    #[test]
    fn ratio_out_of_range_rejected() {
        let f = Filter {
            min_likes_per_impression: Some(1.5),
            ..Default::default()
        };
        assert!(f.validate().is_err());

        let f = Filter {
            max_replies_per_impression: Some(-0.1),
            ..Default::default()
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn ratio_inverted_pair_rejected() {
        let f = Filter {
            min_retweets_per_impression: Some(0.5),
            max_retweets_per_impression: Some(0.1),
            ..Default::default()
        };
        let err = f.validate().unwrap_err();
        assert!(err.contains("min_retweets_per_impression"));
        assert!(err.contains("max_retweets_per_impression"));
    }
}
