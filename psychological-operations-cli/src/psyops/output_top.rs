//! Runtime evaluator for `OutputTop::Starlark`. The variant
//! definition + publish-time `validate` live in the SDK
//! (`stage::parse_output_top`); this file is the CLI's runtime:
//! take the post set surviving up to this point + each post's
//! stage score, build the `tweets` Starlark global, evaluate
//! the operator's expression, coerce the result into a `usize`
//! cap.

use chrono::Utc;
use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::values::float::StarlarkFloat;
use starlark::values::{UnpackValue, Value, ValueLike};

use psychological_operations_sdk::cli::psyops::stage::parse_output_top;

use crate::score::ScoredPost;
use crate::tweet::alloc_post_dict_with_score;

/// Evaluate the operator's Starlark expression and return the
/// integer cap to apply to `posts` (caller does the
/// truncation). Returns `Err(String)` on parse / eval / type /
/// range failure; the caller maps that to `crate::error::Error`.
pub fn evaluate(src: &str, posts: &[ScoredPost]) -> Result<usize, String> {
    let ast = parse_output_top(src)?;
    let module = Module::new();
    let now = Utc::now();
    {
        let heap = module.heap();
        let dicts: Vec<Value> = posts
            .iter()
            .map(|s| alloc_post_dict_with_score(&s.post, s.score, &now, heap))
            .collect();
        module.set("tweets", heap.alloc(dicts));
    }
    let globals = Globals::standard();
    {
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals).map_err(|e| e.to_string())?;
    }
    let result_owned = module
        .get("result")
        .ok_or_else(|| "output_top.starlark produced no result".to_string())?;
    let result = result_owned.to_value();

    coerce_to_count(result)
}

/// Accept a non-negative int directly (any width up to usize),
/// or a finite float that is whole-valued and non-negative.
/// Rejects negatives, NaN/Inf, non-integral floats, and any
/// non-numeric value with a human-readable message.
fn coerce_to_count(v: Value<'_>) -> Result<usize, String> {
    // Int path: `usize` unpacker returns Err on negative / too-large.
    match usize::unpack_value(v) {
        Ok(Some(n)) => return Ok(n),
        Ok(None) => {}
        Err(e) => {
            return Err(format!("output_top.starlark integer out of range: {e}"));
        }
    }
    // Float path: accept whole-valued, non-negative, finite.
    if let Some(f) = StarlarkFloat::unpack_value_opt(v) {
        let f = f.0;
        if !f.is_finite() || f < 0.0 || f.fract() != 0.0 {
            return Err(format!(
                "output_top.starlark returned float {f} — must be a non-negative whole number"
            ));
        }
        return Ok(f as usize);
    }
    Err(format!(
        "output_top.starlark must return an int or whole-number float, got {}",
        v.get_type(),
    ))
}
