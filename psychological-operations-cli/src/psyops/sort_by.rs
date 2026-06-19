//! Runtime sort over the query candidates in a bucket. The `SortBy`
//! enum + publish-time `validate` live in
//! `psychological_operations_sdk::cli::psyops::sort_by`; this file is
//! purely the evaluator. It sorts the items themselves (a projection
//! gives each item's `Tweet`), so there is no tweet-ID round-trip — built-in
//! sorts touch no ID at all, and de-duplication is irrelevant: identical
//! items just sort adjacently.
//!
//! The `Custom` variant runs the operator's Starlark expression, which
//! returns a list of **sort values** positionally aligned to `tweets`:
//! element `i` is the value for tweet `i`. Items sort ascending by value
//! (equal = original order; negate for descending). A `None` value — or a
//! position past the end of a short list — drops that item; extra elements
//! are ignored.

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::values::float::StarlarkFloat;
use starlark::values::list::ListRef;
use starlark::values::{UnpackValue, Value, ValueLike};

use psychological_operations_sdk::cli::psyops::sort_by::{parse_custom, SortBy};

use crate::tweet::{alloc_dict, Tweet};

/// Reorder `items` per the variant's rule, sorting the items themselves
/// (`tweet_of` projects each to the `Tweet` the rule reads). Built-ins are
/// a stable sort over all items. `Custom` sorts by the operator's
/// per-tweet sort values and may also drop items, so its output can be
/// shorter than the input.
pub fn evaluate<T>(
    s: &SortBy,
    mut items: Vec<T>,
    tweet_of: impl Fn(&T) -> &Tweet,
) -> Result<Vec<T>, String> {
    match s {
        SortBy::Likes => {
            items.sort_by(|a, b| tweet_of(b).likes.cmp(&tweet_of(a).likes));
            Ok(items)
        }
        SortBy::Retweets => {
            items.sort_by(|a, b| tweet_of(b).retweets.cmp(&tweet_of(a).retweets));
            Ok(items)
        }
        SortBy::Replies => {
            items.sort_by(|a, b| tweet_of(b).replies.cmp(&tweet_of(a).replies));
            Ok(items)
        }
        SortBy::Newest => {
            items.sort_by(|a, b| tweet_of(b).created.cmp(&tweet_of(a).created));
            Ok(items)
        }
        SortBy::Oldest => {
            items.sort_by(|a, b| tweet_of(a).created.cmp(&tweet_of(b).created));
            Ok(items)
        }
        SortBy::Custom(src) => evaluate_custom(src, items, tweet_of),
    }
}

fn evaluate_custom<T>(
    src: &str,
    items: Vec<T>,
    tweet_of: impl Fn(&T) -> &Tweet,
) -> Result<Vec<T>, String> {
    let ast = parse_custom(src)?;
    let module = Module::new();
    {
        let heap = module.heap();
        let dicts: Vec<Value> = items.iter().map(|t| alloc_dict(tweet_of(t), heap)).collect();
        module.set("tweets", heap.alloc(dicts));
    }
    let globals = Globals::standard();
    {
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals).map_err(|e| e.to_string())?;
    }
    let result_owned = module
        .get("result")
        .ok_or_else(|| "custom sort produced no result".to_string())?;
    let result = result_owned.to_value();
    let list =
        ListRef::from_value(result).ok_or_else(|| "custom sort must return a list".to_string())?;

    // The returned list holds one sort value per tweet, positionally
    // aligned: element i is tweet i's value. Element `None`, or a position
    // past a short list, drops that item; extra elements are ignored.
    let values: Vec<Value> = list.iter().collect();
    let mut keyed: Vec<(f64, T)> = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let value = match values.get(i) {
            Some(v) => extract_sort_value(*v)?,
            None => None, // returned list too short — drop the tail
        };
        if let Some(v) = value {
            keyed.push((v, item));
        }
    }
    // Ascending; equal values keep original order (stable sort + we built
    // `keyed` in input order).
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(keyed.into_iter().map(|(_, it)| it).collect())
}

/// A custom-sort element: a number → its `f64` sort value; `None` → drop
/// this item. Anything else is an error.
fn extract_sort_value(v: Value<'_>) -> Result<Option<f64>, String> {
    if v.is_none() {
        return Ok(None);
    }
    if let Ok(Some(i)) = i64::unpack_value(v) {
        return Ok(Some(i as f64));
    }
    if let Some(f) = StarlarkFloat::unpack_value_opt(v) {
        return Ok(Some(f.0));
    }
    Err(format!(
        "custom sort values must be a number or None, got {}",
        v.get_type(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweet::tw_default;

    fn tw(id: &str, likes: u64) -> Tweet {
        Tweet {
            likes,
            ..tw_default(id)
        }
    }

    fn ids(ts: &[Tweet]) -> Vec<&str> {
        ts.iter().map(|t| t.id.as_str()).collect()
    }

    fn sort(s: &SortBy, v: Vec<Tweet>) -> Result<Vec<Tweet>, String> {
        evaluate(s, v, |t| t)
    }

    #[test]
    fn likes_descending() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let out = sort(&SortBy::Likes, v).unwrap();
        assert_eq!(ids(&out), vec!["b", "c", "a"]);
    }

    #[test]
    fn newest_oldest() {
        let mut a = tw_default("a");
        a.created = "2026-01-01T00:00:00Z".into();
        let mut b = tw_default("b");
        b.created = "2026-05-01T00:00:00Z".into();
        let mut c = tw_default("c");
        c.created = "2026-03-01T00:00:00Z".into();
        let v = vec![a.clone(), b.clone(), c.clone()];
        let newest = sort(&SortBy::Newest, v.clone()).unwrap();
        assert_eq!(ids(&newest), vec!["b", "c", "a"]);
        let oldest = sort(&SortBy::Oldest, v).unwrap();
        assert_eq!(ids(&oldest), vec!["a", "c", "b"]);
    }

    #[test]
    fn custom_values_ascending() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom("[t['likes'] for t in tweets]".into());
        let out = sort(&custom, v).unwrap();
        assert_eq!(ids(&out), vec!["a", "c", "b"]);
    }

    #[test]
    fn custom_negate_for_descending() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom("[-t['likes'] for t in tweets]".into());
        let out = sort(&custom, v).unwrap();
        assert_eq!(ids(&out), vec!["b", "c", "a"]);
    }

    #[test]
    fn custom_none_drops() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom =
            SortBy::Custom("[t['likes'] if t['likes'] > 2 else None for t in tweets]".into());
        let out = sort(&custom, v).unwrap();
        assert_eq!(ids(&out), vec!["c", "b"]); // a (1) dropped; ascending by likes
    }

    #[test]
    fn custom_too_short_drops_tail() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        // Only one value returned → tweets 1 and 2 drop.
        let custom = SortBy::Custom("[t['likes'] for t in tweets][:1]".into());
        let out = sort(&custom, v).unwrap();
        assert_eq!(ids(&out), vec!["a"]);
    }

    #[test]
    fn custom_too_long_ignores_extras() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom("[t['likes'] for t in tweets] + [99, 99]".into());
        let out = sort(&custom, v).unwrap();
        assert_eq!(ids(&out), vec!["a", "c", "b"]);
    }

    #[test]
    fn custom_equal_values_keep_original_order() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom("[0 for t in tweets]".into());
        let out = sort(&custom, v).unwrap();
        assert_eq!(ids(&out), vec!["a", "b", "c"]);
    }

    #[test]
    fn custom_non_number_errors() {
        let v = vec![tw("a", 1), tw("b", 5)];
        let custom = SortBy::Custom("[t['id'] for t in tweets]".into());
        assert!(sort(&custom, v).is_err());
    }

    #[test]
    fn custom_not_a_list_errors() {
        let v = vec![tw("a", 1)];
        let custom = SortBy::Custom("42".into());
        assert!(sort(&custom, v).is_err());
    }
}
