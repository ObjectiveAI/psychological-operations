//! Runtime evaluator for `OutputTop::Python`. The variant definition +
//! publish-time `validate` live in the SDK (`stage`); this file is the CLI's
//! runtime: take the post set surviving up to this point + each post's stage
//! score, build the `input` list of dicts, run the operator's Python
//! expression via the `python` command, and coerce the result into a `usize`
//! cap (the caller does the truncation).

use chrono::Utc;

use crate::error::Error;
use crate::score::ScoredPost;
use crate::tweet::post_with_score_json;

/// Evaluate the operator's Python expression and return the integer cap to
/// apply to `posts`. Errors on eval / type / range failure.
pub async fn evaluate(
    ctx: &crate::context::Context,
    src: &str,
    posts: &[ScoredPost],
) -> Result<usize, Error> {
    let now = Utc::now();
    let input = serde_json::Value::Array(
        posts
            .iter()
            .map(|s| post_with_score_json(&s.post, s.score, &now))
            .collect(),
    );
    let result = crate::psyops::pyeval::run(ctx, src, input).await?;
    coerce_to_count(&result)
}

/// Accept a non-negative int directly, or a finite float that is
/// whole-valued and non-negative. Rejects negatives, NaN/Inf, non-integral
/// floats, and any non-numeric value with a human-readable message.
fn coerce_to_count(v: &serde_json::Value) -> Result<usize, Error> {
    if let Some(u) = v.as_u64() {
        return usize::try_from(u)
            .map_err(|e| Error::Other(format!("output_top integer out of range: {e}")));
    }
    if let Some(f) = v.as_f64() {
        if !f.is_finite() || f < 0.0 || f.fract() != 0.0 {
            return Err(Error::Other(format!(
                "output_top returned {f} — must be a non-negative whole number"
            )));
        }
        return Ok(f as usize);
    }
    Err(Error::Other(format!(
        "output_top must return an int or whole-number float, got {v}"
    )))
}
