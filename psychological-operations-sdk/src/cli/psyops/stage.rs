use objectiveai_sdk::functions::executions::request::Strategy;
use objectiveai_sdk::functions::{
    AlphaInlineFunction, FullInlineFunction, FullInlineFunctionOrRemoteCommitOptional,
    InlineFunction, InlineProfileOrRemoteCommitOptional,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starlark::syntax::{AstModule, Dialect};

/// The one knob every `Stage` variant has in common: after the
/// stage produces its scores, narrow the surviving set with an
/// optional cap or fraction. Everything function-specific
/// (threshold, invert, images/videos hints, the function /
/// profile / strategy triple itself) lives only on
/// [`Stage::Function`] — `bare` would have no use for those.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StageBase {
    /// Narrow the surviving set. Tagged:
    /// `{"type":"fixed","value":10}` or
    /// `{"type":"fraction","value":0.25}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_top: Option<OutputTop>,
}

/// One pass over the surviving post set. Tagged on `"type"`:
///
/// - **`bare`** — pass-through. Assigns every post a flat 1.0
///   score and applies the shared `output_top` cap. No
///   objectiveai call, no threshold (1.0 either always passes
///   or always fails an arbitrary threshold), no per-call
///   invert / images / videos hints.
/// - **`function`** — the existing objectiveai scoring shape:
///   function + profile + strategy + invert + images / videos
///   + output_threshold + the shared output_top. The shared
///   `output_top` lands at top-level JSON via
///   `#[serde(flatten)]` on the embedded `StageBase`.
///
/// PsyOp carries a `Vec<Stage>` so multi-stage pipelines can
/// mix the two — e.g., a bare pass-through with a `Fixed(20)`
/// cap as stage 0, then a function pass to score those 20 as
/// stage 1.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Stage {
    Bare {
        #[serde(flatten)]
        base: StageBase,
    },
    Function {
        #[serde(flatten)]
        base: StageBase,
        function: FullInlineFunctionOrRemoteCommitOptional,
        profile: InlineProfileOrRemoteCommitOptional,
        strategy: Strategy,
        /// Inverts the scoring result (`1 - score`) before
        /// threshold / top narrowing apply.
        #[serde(default)]
        invert: bool,
        /// If `false`, scored posts are sent to the function
        /// with an empty `images` array regardless of what was
        /// ingested. Defaults to `true`.
        #[serde(default = "default_true")]
        images: bool,
        /// If `false`, scored posts are sent to the function
        /// with an empty `videos` array regardless of what was
        /// ingested. Defaults to `true`.
        #[serde(default = "default_true")]
        videos: bool,
        /// Drop posts scoring below this threshold before they
        /// advance to the next stage (or are returned as final
        /// output if this is the last stage). Range `[0.0, 1.0]`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_threshold: Option<f64>,
    },
}

/// How to narrow a stage's output before it advances. Adjacent-
/// tagged on `"type"` + `"value"` so an agent constructing a
/// Stage body has a clear, schema-discoverable contract for
/// which payload shape applies.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum OutputTop {
    /// Keep the top `value` posts after scoring (and, for the
    /// function variant, after `output_threshold` filtering).
    /// Acts as an absolute cap; if the surviving set is smaller
    /// than `value`, everything passes through.
    Fixed(u64),
    /// Keep the top `ceil(N · value)` posts. `value` is in
    /// `[0.0, 1.0]` — e.g. `0.25` = top quarter.
    Fraction(f64),
    /// Starlark expression. Receives one global, `tweets` — a
    /// list of dicts mirroring `Tweet` (keys `id`, `handle`,
    /// `created`, `age`, `likes`, `retweets`, `replies`,
    /// `impressions`) plus `score: float` (the just-computed
    /// stage score). Must evaluate to a non-negative integer
    /// (or a float that round-trips cleanly to one). The result
    /// is the absolute cap, same semantics as `Fixed`.
    Starlark(String),
}

fn default_true() -> bool {
    true
}

/// Parse-only check on `OutputTop::Starlark`. Called by
/// `StageBase::validate` so a bad expression is rejected at
/// publish time, not at run time. Mirrors `sort_by::parse_custom`
/// — wrap as `result = (expr)\n` so the CLI-side evaluator can
/// pull the bound value back out of the module.
pub fn parse_output_top(src: &str) -> Result<AstModule, String> {
    let wrapped = format!("result = ({src})\n");
    AstModule::parse("output_top.starlark", wrapped, &Dialect::Standard).map_err(|e| e.to_string())
}

impl Stage {
    /// Borrow the shared `StageBase` fields without matching on
    /// the variant. Useful for callers that just need
    /// `output_top`.
    pub fn base(&self) -> &StageBase {
        match self {
            Stage::Bare { base } => base,
            Stage::Function { base, .. } => base,
        }
    }

    /// Publish-time consistency check. Validates the shared
    /// `StageBase`'s `output_top`, plus the function variant's
    /// `output_threshold`.
    pub fn validate(&self) -> Result<(), String> {
        self.base().validate()?;
        if let Stage::Function {
            output_threshold: Some(t),
            ..
        } = self
        {
            if !t.is_finite() || !(0.0..=1.0).contains(t) {
                return Err(format!("output_threshold ({t}) must be in [0.0, 1.0]"));
            }
        }
        Ok(())
    }
}

impl StageBase {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(top) = &self.output_top {
            match top {
                OutputTop::Fraction(p) => {
                    if !p.is_finite() || !(0.0..=1.0).contains(p) {
                        return Err(format!(
                            "output_top.value ({p}) must be in [0.0, 1.0] for fraction"
                        ));
                    }
                }
                OutputTop::Fixed(_) => {
                    // u64 already constrains to >= 0; no further
                    // check. Fixed(0) is allowed (means "drop
                    // everything"), matching Fraction(0.0).
                }
                OutputTop::Starlark(src) => {
                    parse_output_top(src).map(|_| ())?;
                }
            }
        }
        Ok(())
    }
}

/// Determine if a function is a vector function. If the function
/// is remote, it must be fetched first (caller resolves it).
pub fn is_vector_function(function: &FullInlineFunction) -> bool {
    match function {
        FullInlineFunction::Alpha(alpha) => matches!(alpha, AlphaInlineFunction::Vector(_)),
        FullInlineFunction::Standard(standard) => matches!(standard, InlineFunction::Vector { .. }),
    }
}
