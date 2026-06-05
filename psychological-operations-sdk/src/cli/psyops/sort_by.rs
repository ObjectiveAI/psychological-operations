use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use starlark::syntax::{AstModule, Dialect};

/// Tiebreak order applied across the deduped candidate union when
/// truncating to `PsyOp.max_posts`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema)]
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
}

/// Parse a SortBy `Custom` Starlark expression into an AST module.
/// Exposed `pub` so the CLI-side evaluator can re-use the same
/// parser without duplicating it.
pub fn parse_custom(src: &str) -> Result<AstModule, String> {
    // Bind the expression to a public name (no leading underscore)
    // — starlark hides any module global whose name starts with `_`.
    let wrapped = format!("result = ({src})\n");
    AstModule::parse("sort.custom", wrapped, &Dialect::Standard)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_syntax_error_at_validate() {
        let s = SortBy::Custom("sorted(tweets,".into());
        assert!(s.validate().is_err());
    }
}
