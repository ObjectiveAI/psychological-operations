//! Runtime evaluation against live [`Tweet`] rows. The `Filter`
//! struct + publish-time `validate` live in
//! `psychological_operations_sdk::cli::psyops::filter`; this file
//! is purely the evaluator (uses Starlark to run the operator's
//! `custom` expression against a tweet's metrics).

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::values::ValueLike;

use psychological_operations_sdk::cli::psyops::filter::{parse_custom, Filter};

use crate::tweet::Tweet;

/// Returns `Ok(true)` iff every static `min_*` / `max_*` gate
/// passes AND, when present, the `custom` Starlark expression
/// evaluates to `True`. Returns `Ok(false)` if any static gate
/// rejects. Returns `Err` on Starlark parse / eval / type
/// errors.
///
/// Static gates run first (cheap) so a tweet that's already
/// rejected on engagement counts never pays the Starlark cost.
pub fn evaluate(f: &Filter, t: &Tweet) -> Result<bool, String> {
    if !static_pass(f, t) {
        return Ok(false);
    }
    match &f.custom {
        None => Ok(true),
        Some(src) => evaluate_custom(src, t),
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
        assert!(evaluate(&f, &tw(200, 0, 0, 0, 0)).unwrap());
        assert!(!evaluate(&f, &tw(50, 0, 0, 0, 0)).unwrap());
    }

    #[test]
    fn custom_combines_with_static() {
        let f = Filter {
            min_likes: Some(10),
            custom: Some("retweets > replies".into()),
            ..Default::default()
        };
        assert!(evaluate(&f, &tw(20, 5, 1, 0, 0)).unwrap());   // both pass
        assert!(!evaluate(&f, &tw(20, 1, 5, 0, 0)).unwrap());  // custom fails
        assert!(!evaluate(&f, &tw(5,  5, 1, 0, 0)).unwrap());  // static fails
    }

    #[test]
    fn non_bool_result_is_an_error() {
        let f = Filter {
            custom: Some("42".into()),
            ..Default::default()
        };
        assert!(evaluate(&f, &tw(0, 0, 0, 0, 0)).is_err());
    }

    #[test]
    fn ratio_gates_apply() {
        // min 0.05 likes/impression — 5%+ engagement only
        let f = Filter {
            min_likes_per_impression: Some(0.05),
            ..Default::default()
        };
        assert!(evaluate(&f, &tw(60, 0, 0, 1000, 0)).unwrap());   // 6% — pass
        assert!(!evaluate(&f, &tw(40, 0, 0, 1000, 0)).unwrap());  // 4% — reject
        // zero impressions: ratio gates are skipped entirely, so this
        // passes despite the positive `min_likes_per_impression`.
        assert!(evaluate(&f, &tw(10, 0, 0, 0, 0)).unwrap());
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
        assert!(evaluate(&f, &tw(1, 2, 3, 4, 5)).unwrap());
    }
}
