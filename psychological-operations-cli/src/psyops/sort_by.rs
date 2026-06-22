//! Runtime sort over the query candidates in a bucket. The `SortBy`
//! enum + publish-time `validate` live in
//! `psychological_operations_sdk::cli::psyops::sort_by`; this file is
//! purely the evaluator. It sorts the items themselves (a projection
//! gives each item's `Tweet`), so there is no tweet-ID round-trip — built-in
//! sorts touch no ID at all, and de-duplication is irrelevant: identical
//! items just sort adjacently.
//!
//! The `Python` variant runs the operator's Python expression (via the
//! `python` command). Its `input` global is the list of tweet dicts (in
//! candidate order) and its trailing expression returns a list of **sort
//! values** positionally aligned to `input`: element `i` is the value for
//! tweet `i`. Items sort ascending by value (equal = original order; negate
//! for descending). A `None` value — or a position past the end of a short
//! list — drops that item; extra elements are ignored.

use psychological_operations_sdk::cli::psyops::sort_by::SortBy;

use crate::error::Error;
use crate::tweet::{tweet_json, Tweet};

/// Reorder `items` per the variant's rule, sorting the items themselves
/// (`tweet_of` projects each to the `Tweet` the rule reads). Built-ins are
/// a stable sort over all items. `Python` sorts by the operator's
/// per-tweet sort values and may also drop items, so its output can be
/// shorter than the input.
pub async fn evaluate<T>(
    ctx: &crate::context::Context,
    s: &SortBy,
    items: Vec<T>,
    tweet_of: impl Fn(&T) -> &Tweet,
) -> Result<Vec<T>, Error> {
    match s {
        SortBy::Python(src) => evaluate_python(ctx, src, items, tweet_of).await,
        builtin => Ok(sort_builtin(builtin, items, tweet_of)),
    }
}

/// The five built-in stable sorts (no host needed). The `Python` arm is
/// unreachable — [`evaluate`] routes it away — and returns the input as-is.
fn sort_builtin<T>(s: &SortBy, mut items: Vec<T>, tweet_of: impl Fn(&T) -> &Tweet) -> Vec<T> {
    match s {
        SortBy::Likes => {
            items.sort_by(|a, b| tweet_of(b).likes.cmp(&tweet_of(a).likes));
        }
        SortBy::Retweets => {
            items.sort_by(|a, b| tweet_of(b).retweets.cmp(&tweet_of(a).retweets));
        }
        SortBy::Replies => {
            items.sort_by(|a, b| tweet_of(b).replies.cmp(&tweet_of(a).replies));
        }
        SortBy::Newest => {
            items.sort_by(|a, b| tweet_of(b).created.cmp(&tweet_of(a).created));
        }
        SortBy::Oldest => {
            items.sort_by(|a, b| tweet_of(a).created.cmp(&tweet_of(b).created));
        }
        SortBy::Python(_) => {}
    }
    items
}

async fn evaluate_python<T>(
    ctx: &crate::context::Context,
    src: &str,
    items: Vec<T>,
    tweet_of: impl Fn(&T) -> &Tweet,
) -> Result<Vec<T>, Error> {
    let input =
        serde_json::Value::Array(items.iter().map(|t| tweet_json(tweet_of(t))).collect());
    let result = crate::psyops::pyeval::run(ctx, src, input).await?;
    let values = result
        .as_array()
        .ok_or_else(|| Error::Other("custom sort must return a list".into()))?;

    // The returned list holds one sort value per tweet, positionally
    // aligned: element i is tweet i's value. Element `null`, or a position
    // past a short list, drops that item; extra elements are ignored.
    let mut keyed: Vec<(f64, T)> = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let value = match values.get(i) {
            Some(v) => extract_sort_value(v)?,
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

/// A custom-sort element: a number → its `f64` sort value; `null` → drop
/// this item. Anything else is an error.
fn extract_sort_value(v: &serde_json::Value) -> Result<Option<f64>, Error> {
    if v.is_null() {
        return Ok(None);
    }
    if let Some(f) = v.as_f64() {
        return Ok(Some(f));
    }
    Err(Error::Other(format!(
        "custom sort values must be a number or null, got {v}"
    )))
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

    // Built-in sorts are sync + host-free; the `Python` path runs against the
    // host's python runtime and is exercised by the integration suite.
    fn sort(s: &SortBy, v: Vec<Tweet>) -> Vec<Tweet> {
        sort_builtin(s, v, |t| t)
    }

    #[test]
    fn likes_descending() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        assert_eq!(ids(&sort(&SortBy::Likes, v)), vec!["b", "c", "a"]);
    }

    #[test]
    fn newest_oldest() {
        let mut a = tw_default("a");
        a.created = "2026-01-01T00:00:00Z".into();
        let mut b = tw_default("b");
        b.created = "2026-05-01T00:00:00Z".into();
        let mut c = tw_default("c");
        c.created = "2026-03-01T00:00:00Z".into();
        let v = vec![a, b, c];
        assert_eq!(ids(&sort(&SortBy::Newest, v.clone())), vec!["b", "c", "a"]);
        assert_eq!(ids(&sort(&SortBy::Oldest, v)), vec!["a", "c", "b"]);
    }
}
