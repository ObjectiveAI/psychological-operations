use serde::{Deserialize, Serialize};

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::ValueLike;

use crate::tweet::Tweet;

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
    ///   - For every `min_X` / `max_X` pair (counts and ratios),
    ///     both bounds must be consistent (`min <= max`) when both
    ///     are set.
    ///   - Every `_per_impression` ratio bound must lie in `[0, 1]`.
    ///   - If `custom` is `Some`, the Starlark expression must parse.
    pub fn validate(&self) -> Result<(), String> {
        check_pair("likes",       self.min_likes,       self.max_likes)?;
        check_pair("retweets",    self.min_retweets,    self.max_retweets)?;
        check_pair("replies",     self.min_replies,     self.max_replies)?;
        check_pair("impressions", self.min_impressions, self.max_impressions)?;
        check_pair("age",         self.min_age,         self.max_age)?;

        check_ratio("min_likes_per_impression",     self.min_likes_per_impression)?;
        check_ratio("max_likes_per_impression",     self.max_likes_per_impression)?;
        check_ratio("min_retweets_per_impression",  self.min_retweets_per_impression)?;
        check_ratio("max_retweets_per_impression",  self.max_retweets_per_impression)?;
        check_ratio("min_replies_per_impression",   self.min_replies_per_impression)?;
        check_ratio("max_replies_per_impression",   self.max_replies_per_impression)?;

        check_ratio_pair("likes_per_impression",
            self.min_likes_per_impression,    self.max_likes_per_impression)?;
        check_ratio_pair("retweets_per_impression",
            self.min_retweets_per_impression, self.max_retweets_per_impression)?;
        check_ratio_pair("replies_per_impression",
            self.min_replies_per_impression,  self.max_replies_per_impression)?;

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
    pub fn evaluate(&self, t: &Tweet) -> Result<bool, String> {
        if !static_pass(self, t) {
            return Ok(false);
        }
        match &self.custom {
            None => Ok(true),
            Some(src) => evaluate_custom(src, t),
        }
    }
}

fn static_pass(f: &Filter, t: &Tweet) -> bool {
    if let Some(v) = f.min_likes        { if t.likes       < v { return false; } }
    if let Some(v) = f.max_likes        { if t.likes       > v { return false; } }
    if let Some(v) = f.min_retweets     { if t.retweets    < v { return false; } }
    if let Some(v) = f.max_retweets     { if t.retweets    > v { return false; } }
    if let Some(v) = f.min_replies      { if t.replies     < v { return false; } }
    if let Some(v) = f.max_replies      { if t.replies     > v { return false; } }
    if let Some(v) = f.min_impressions  { if t.impressions < v { return false; } }
    if let Some(v) = f.max_impressions  { if t.impressions > v { return false; } }
    if let Some(v) = f.min_age          { if t.age         < v { return false; } }
    if let Some(v) = f.max_age          { if t.age         > v { return false; } }

    // Per-impression ratio gates are skipped entirely when
    // impressions == 0 — there's no meaningful rate without a
    // denominator, and we don't want to silently reject rows just
    // because the impression count hasn't been observed yet.
    if t.impressions > 0 {
        let denom = t.impressions as f64;
        let likes_pi    = t.likes    as f64 / denom;
        let retweets_pi = t.retweets as f64 / denom;
        let replies_pi  = t.replies  as f64 / denom;
        if let Some(v) = f.min_likes_per_impression    { if likes_pi    < v { return false; } }
        if let Some(v) = f.max_likes_per_impression    { if likes_pi    > v { return false; } }
        if let Some(v) = f.min_retweets_per_impression { if retweets_pi < v { return false; } }
        if let Some(v) = f.max_retweets_per_impression { if retweets_pi > v { return false; } }
        if let Some(v) = f.min_replies_per_impression  { if replies_pi  < v { return false; } }
        if let Some(v) = f.max_replies_per_impression  { if replies_pi  > v { return false; } }
    }
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
            return Err(format!(
                "min_{name} ({lo}) must be <= max_{name} ({hi})",
            ));
        }
    }
    Ok(())
}

fn parse_custom(src: &str) -> Result<AstModule, String> {
    // Bind the expression to a public name (no leading underscore)
    // — starlark hides any module global whose name starts with `_`.
    let wrapped = format!("result = ({src})\n");
    AstModule::parse("filter.custom", wrapped, &Dialect::Standard)
        .map_err(|e| e.to_string())
}

fn evaluate_custom(src: &str, t: &Tweet) -> Result<bool, String> {
    let ast = parse_custom(src)?;
    let module = Module::new();
    {
        let heap = module.heap();
        module.set("likes",       heap.alloc(t.likes as i64));
        module.set("retweets",    heap.alloc(t.retweets as i64));
        module.set("replies",     heap.alloc(t.replies as i64));
        module.set("impressions", heap.alloc(t.impressions as i64));
        module.set("age",         heap.alloc(t.age as i64));
    }
    let globals = Globals::standard();
    {
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|e| e.to_string())?;
    }
    let value = module
        .get("result")
        .ok_or_else(|| "custom expression produced no result".to_string())?;
    value
        .to_value()
        .unpack_bool()
        .ok_or_else(|| "custom expression must evaluate to bool".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweet::tw_default;

    fn tw(likes: u64, retweets: u64, replies: u64, impressions: u64, age: u64) -> Tweet {
        Tweet { likes, retweets, replies, impressions, age, ..tw_default("test") }
    }

    #[test]
    fn custom_passes_and_fails() {
        let f = Filter {
            custom: Some("likes > 100".into()),
            ..Default::default()
        };
        assert!(f.evaluate(&tw(200, 0, 0, 0, 0)).unwrap());
        assert!(!f.evaluate(&tw(50, 0, 0, 0, 0)).unwrap());
    }

    #[test]
    fn custom_combines_with_static() {
        let f = Filter {
            min_likes: Some(10),
            custom: Some("retweets > replies".into()),
            ..Default::default()
        };
        assert!(f.evaluate(&tw(20, 5, 1, 0, 0)).unwrap());   // both pass
        assert!(!f.evaluate(&tw(20, 1, 5, 0, 0)).unwrap());  // custom fails
        assert!(!f.evaluate(&tw(5,  5, 1, 0, 0)).unwrap());  // static fails
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
        assert!(f.evaluate(&tw(0, 0, 0, 0, 0)).is_err());
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

    #[test]
    fn ratio_gates_apply() {
        // min 0.05 likes/impression — 5%+ engagement only
        let f = Filter {
            min_likes_per_impression: Some(0.05),
            ..Default::default()
        };
        f.validate().unwrap();
        assert!(f.evaluate(&tw(60, 0, 0, 1000, 0)).unwrap());   // 6% — pass
        assert!(!f.evaluate(&tw(40, 0, 0, 1000, 0)).unwrap());  // 4% — reject
        // zero impressions: ratio gates are skipped entirely, so this
        // passes despite the positive `min_likes_per_impression`.
        assert!(f.evaluate(&tw(10, 0, 0, 0, 0)).unwrap());
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
        assert!(f.evaluate(&tw(1, 2, 3, 4, 5)).unwrap());
    }
}
