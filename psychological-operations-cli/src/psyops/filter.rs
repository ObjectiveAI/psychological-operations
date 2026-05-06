use serde::{Deserialize, Serialize};

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::ValueLike;

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
/// `custom` is an optional Starlark boolean expression that
/// AND-combines with the static gates above. See `evaluate`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Filter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_likes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_likes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_retweets: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retweets: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_replies: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_replies: Option<u64>,
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
    /// Optional Starlark boolean expression. Receives `likes`,
    /// `retweets`, `replies`, `impressions`, `age` (all `int`, age
    /// in seconds). Must evaluate to `bool` — non-bool results are
    /// rejected as errors, not coerced. AND-combines with the
    /// static gates above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

impl Filter {
    /// Validate the filter at publish time:
    ///   - For every `min_X` / `max_X` pair, both bounds must be
    ///     consistent (`min <= max`) when both are set.
    ///   - If `custom` is `Some`, the Starlark expression must parse.
    pub fn validate(&self) -> Result<(), String> {
        check_pair("likes",       self.min_likes,       self.max_likes)?;
        check_pair("retweets",    self.min_retweets,    self.max_retweets)?;
        check_pair("replies",     self.min_replies,     self.max_replies)?;
        check_pair("impressions", self.min_impressions, self.max_impressions)?;
        check_pair("age",         self.min_age,         self.max_age)?;
        if let Some(src) = &self.custom {
            parse_custom(src).map(|_| ())?;
        }
        Ok(())
    }

    /// Returns `Ok(true)` iff every static `min_*` / `max_*` gate
    /// passes AND, when present, the `custom` Starlark expression
    /// evaluates to `True`. Returns `Ok(false)` if any static gate
    /// rejects. Returns `Err` on Starlark parse / eval / type
    /// errors.
    ///
    /// Static gates run first (cheap) so a tweet that's already
    /// rejected on engagement counts never pays the Starlark cost.
    pub fn evaluate(
        &self,
        likes: u64,
        retweets: u64,
        replies: u64,
        impressions: u64,
        age_secs: u64,
    ) -> Result<bool, String> {
        if !static_pass(self, likes, retweets, replies, impressions, age_secs) {
            return Ok(false);
        }
        match &self.custom {
            None => Ok(true),
            Some(src) => evaluate_custom(src, likes, retweets, replies, impressions, age_secs),
        }
    }
}

fn static_pass(
    f: &Filter,
    likes: u64,
    retweets: u64,
    replies: u64,
    impressions: u64,
    age: u64,
) -> bool {
    if let Some(v) = f.min_likes        { if likes < v        { return false; } }
    if let Some(v) = f.max_likes        { if likes > v        { return false; } }
    if let Some(v) = f.min_retweets     { if retweets < v     { return false; } }
    if let Some(v) = f.max_retweets     { if retweets > v     { return false; } }
    if let Some(v) = f.min_replies      { if replies < v      { return false; } }
    if let Some(v) = f.max_replies      { if replies > v      { return false; } }
    if let Some(v) = f.min_impressions  { if impressions < v  { return false; } }
    if let Some(v) = f.max_impressions  { if impressions > v  { return false; } }
    if let Some(v) = f.min_age          { if age < v          { return false; } }
    if let Some(v) = f.max_age          { if age > v          { return false; } }
    true
}

fn check_pair(name: &str, min: Option<u64>, max: Option<u64>) -> Result<(), String> {
    if let (Some(lo), Some(hi)) = (min, max) {
        if lo > hi {
            return Err(format!(
                "min_{name} ({lo}) must be <= max_{name} ({hi})",
            ));
        }
    }
    Ok(())
}

fn parse_custom(src: &str) -> Result<AstModule, String> {
    let wrapped = format!("_result = ({src})\n");
    AstModule::parse("filter.custom", wrapped, &Dialect::Standard)
        .map_err(|e| e.to_string())
}

fn evaluate_custom(
    src: &str,
    likes: u64,
    retweets: u64,
    replies: u64,
    impressions: u64,
    age: u64,
) -> Result<bool, String> {
    let ast = parse_custom(src)?;
    let module = Module::new();
    {
        let heap = module.heap();
        module.set("likes",       heap.alloc(likes as i64));
        module.set("retweets",    heap.alloc(retweets as i64));
        module.set("replies",     heap.alloc(replies as i64));
        module.set("impressions", heap.alloc(impressions as i64));
        module.set("age",         heap.alloc(age as i64));
    }
    let globals = Globals::standard();
    {
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|e| e.to_string())?;
    }
    let value = module
        .get("_result")
        .ok_or_else(|| "custom expression produced no result".to_string())?;
    value
        .to_value()
        .unpack_bool()
        .ok_or_else(|| "custom expression must evaluate to bool".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_passes_and_fails() {
        let f = Filter {
            custom: Some("likes > 100".into()),
            ..Default::default()
        };
        assert!(f.evaluate(200, 0, 0, 0, 0).unwrap());
        assert!(!f.evaluate(50, 0, 0, 0, 0).unwrap());
    }

    #[test]
    fn custom_combines_with_static() {
        let f = Filter {
            min_likes: Some(10),
            custom: Some("retweets > replies".into()),
            ..Default::default()
        };
        assert!(f.evaluate(20, 5, 1, 0, 0).unwrap());   // both pass
        assert!(!f.evaluate(20, 1, 5, 0, 0).unwrap());  // custom fails
        assert!(!f.evaluate(5,  5, 1, 0, 0).unwrap());  // static fails
    }

    #[test]
    fn syntax_error_at_validate_time() {
        let f = Filter {
            custom: Some("likes >>".into()),
            ..Default::default()
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn non_bool_result_is_an_error() {
        let f = Filter {
            custom: Some("42".into()),
            ..Default::default()
        };
        assert!(f.evaluate(0, 0, 0, 0, 0).is_err());
    }

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
        let f = Filter { min_age: Some(60), ..Default::default() };
        f.validate().unwrap();
    }

    #[test]
    fn five_params_all_bind() {
        let f = Filter {
            custom: Some(
                "likes == 1 and retweets == 2 and replies == 3 and impressions == 4 and age == 5"
                    .into(),
            ),
            ..Default::default()
        };
        assert!(f.evaluate(1, 2, 3, 4, 5).unwrap());
    }
}
