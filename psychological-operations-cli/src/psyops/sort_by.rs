use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::Value;
use starlark::values::ValueLike;
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;

use crate::tweet::{Tweet, alloc_dict};

/// Tiebreak order applied across the deduped candidate union when
/// truncating to `PsyOp.max_posts`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    Likes,
    Retweets,
    Replies,
    Newest,
    Oldest,
    /// Starlark expression. Receives one global, `tweets` — a list
    /// of dicts mirroring `Tweet` (keys: `id`, `handle`, `created`,
    /// `age`, `likes`, `retweets`, `replies`, `impressions`). Must
    /// evaluate to a list whose length matches `tweets` and whose
    /// elements are either dicts (with `id`) or strings (the id).
    /// The returned ordering is the new `Vec<Tweet>` order.
    Custom(String),
}

impl SortBy {
    /// Parse-only check on the Custom variant. Called by
    /// `PsyOp::validate` so a bad expression is rejected at publish
    /// time, not at sort time.
    pub fn validate(&self) -> Result<(), String> {
        if let SortBy::Custom(src) = self {
            parse_custom(src).map(|_| ())?;
        }
        Ok(())
    }

    /// Reorder `tweets` per the variant's rule. Built-ins use a
    /// stable sort; Custom runs the user's Starlark expression and
    /// reorders by the resulting id list.
    pub fn evaluate(&self, mut tweets: Vec<Tweet>) -> Result<Vec<Tweet>, String> {
        match self {
            SortBy::Likes    => { tweets.sort_by(|a, b| b.likes.cmp(&a.likes));       Ok(tweets) }
            SortBy::Retweets => { tweets.sort_by(|a, b| b.retweets.cmp(&a.retweets)); Ok(tweets) }
            SortBy::Replies  => { tweets.sort_by(|a, b| b.replies.cmp(&a.replies));   Ok(tweets) }
            SortBy::Newest   => { tweets.sort_by(|a, b| b.created.cmp(&a.created));   Ok(tweets) }
            SortBy::Oldest   => { tweets.sort_by(|a, b| a.created.cmp(&b.created));   Ok(tweets) }
            SortBy::Custom(src) => evaluate_custom(src, tweets),
        }
    }
}

fn parse_custom(src: &str) -> Result<AstModule, String> {
    // Bind the expression to a public name (no leading underscore)
    // — starlark hides any module global whose name starts with `_`.
    let wrapped = format!("result = ({src})\n");
    AstModule::parse("sort.custom", wrapped, &Dialect::Standard)
        .map_err(|e| e.to_string())
}

fn evaluate_custom(src: &str, tweets: Vec<Tweet>) -> Result<Vec<Tweet>, String> {
    let ast = parse_custom(src)?;
    let module = Module::new();
    {
        let heap = module.heap();
        let dicts: Vec<Value> = tweets.iter().map(|t| alloc_dict(t, heap)).collect();
        module.set("tweets", heap.alloc(dicts));
    }
    let globals = Globals::standard();
    {
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|e| e.to_string())?;
    }
    let result_owned = module
        .get("result")
        .ok_or_else(|| "custom sort produced no result".to_string())?;
    let result = result_owned.to_value();

    let list = ListRef::from_value(result)
        .ok_or_else(|| "custom sort must return a list".to_string())?;

    if list.len() != tweets.len() {
        return Err(format!(
            "custom sort returned {} items but input had {}",
            list.len(),
            tweets.len(),
        ));
    }

    // Build id -> tweet lookup by consuming the input vec exactly
    // once. Duplicate ids in the input would shouldn't happen — the
    // runtime dedupes before calling — but cheap to guard.
    let mut by_id: HashMap<String, Tweet> = HashMap::with_capacity(tweets.len());
    for t in tweets {
        if by_id.insert(t.id.clone(), t).is_some() {
            return Err("duplicate id in input to custom sort".into());
        }
    }

    let mut out = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let id = extract_id(item).ok_or_else(|| {
            format!(
                "custom sort returned element [{i}] that is neither a dict with `id` nor a string",
            )
        })?;
        let tweet = by_id.remove(&id).ok_or_else(|| {
            format!("custom sort returned id `{id}` which was not in the input or was used twice")
        })?;
        out.push(tweet);
    }
    Ok(out)
}

fn extract_id(v: Value<'_>) -> Option<String> {
    if let Some(s) = v.unpack_str() {
        return Some(s.to_string());
    }
    if let Some(dict) = DictRef::from_value(v) {
        for (k, val) in dict.iter() {
            if k.unpack_str() == Some("id") {
                return val.unpack_str().map(|s| s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweet::tw_default;

    fn tw(id: &str, likes: u64) -> Tweet {
        Tweet { likes, ..tw_default(id) }
    }

    fn ids(ts: &[Tweet]) -> Vec<&str> {
        ts.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn likes_descending() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let out = SortBy::Likes.evaluate(v).unwrap();
        assert_eq!(ids(&out), vec!["b", "c", "a"]);
    }

    #[test]
    fn newest_oldest() {
        let mut a = tw_default("a"); a.created = "2026-01-01T00:00:00Z".into();
        let mut b = tw_default("b"); b.created = "2026-05-01T00:00:00Z".into();
        let mut c = tw_default("c"); c.created = "2026-03-01T00:00:00Z".into();
        let v = vec![a.clone(), b.clone(), c.clone()];
        let newest = SortBy::Newest.evaluate(v.clone()).unwrap();
        assert_eq!(ids(&newest), vec!["b", "c", "a"]);
        let oldest = SortBy::Oldest.evaluate(v).unwrap();
        assert_eq!(ids(&oldest), vec!["a", "c", "b"]);
    }

    #[test]
    fn custom_sorted_by_likes_matches_builtin() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom(
            "sorted(tweets, key=lambda t: t['likes'], reverse=True)".into(),
        );
        let out = custom.evaluate(v).unwrap();
        assert_eq!(ids(&out), vec!["b", "c", "a"]);
    }

    #[test]
    fn custom_returning_ids_works() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom(
            "[t['id'] for t in sorted(tweets, key=lambda t: t['likes'])]".into(),
        );
        let out = custom.evaluate(v).unwrap();
        assert_eq!(ids(&out), vec!["a", "c", "b"]);
    }

    #[test]
    fn custom_length_mismatch_errors() {
        let v = vec![tw("a", 1), tw("b", 5), tw("c", 3)];
        let custom = SortBy::Custom("[t for t in tweets if t['likes'] > 2]".into());
        assert!(custom.evaluate(v).is_err());
    }

    #[test]
    fn custom_unknown_id_errors() {
        let v = vec![tw("a", 1), tw("b", 5)];
        let custom = SortBy::Custom("[\"a\", \"missing\"]".into());
        assert!(custom.evaluate(v).is_err());
    }

    #[test]
    fn custom_syntax_error_at_validate() {
        let s = SortBy::Custom("sorted(tweets,".into());
        assert!(s.validate().is_err());
    }
}
