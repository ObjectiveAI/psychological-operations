use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use objectiveai_sdk::functions::{
    FullInlineFunctionOrRemoteCommitOptional,
    FullInlineFunction,
    AlphaInlineFunction,
    InlineFunction,
    InlineProfileOrRemoteCommitOptional,
};
use objectiveai_sdk::functions::executions::request::Strategy;

/// One ObjectiveAI scoring pass over the posts a psyop has assembled
/// (or the surviving subset from the previous stage's output).
/// PsyOp carries a `Vec<Stage>` so multi-stage pipelines are the
/// natural shape: each stage's output, narrowed by `output_threshold`
/// and/or `output_top`, becomes the next stage's input.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Stage {
    pub function: FullInlineFunctionOrRemoteCommitOptional,
    pub profile: InlineProfileOrRemoteCommitOptional,
    pub strategy: Strategy,
    #[serde(default)]
    pub invert: bool,
    /// If `false`, scored posts are sent to the function with an
    /// empty `images` array regardless of what was ingested.
    /// Defaults to `true`.
    #[serde(default = "default_true")]
    pub images: bool,
    /// If `false`, scored posts are sent to the function with an
    /// empty `videos` array regardless of what was ingested.
    /// Defaults to `true`.
    #[serde(default = "default_true")]
    pub videos: bool,
    /// Drop posts scoring below this threshold before they advance
    /// to the next stage (or are returned as final output if this
    /// is the last stage). Range `[0.0, 1.0]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_threshold: Option<f64>,
    /// After applying `output_threshold` (if any), narrow the
    /// surviving set before advancing. Tagged so an agent can
    /// pick between a `fixed` absolute cap (`{ "type": "fixed",
    /// "value": 10 }`) and a `fraction` of the bucket
    /// (`{ "type": "fraction", "value": 0.25 }`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_top: Option<OutputTop>,
}

/// How to narrow a stage's output before it advances. Adjacent-
/// tagged on `"type"` + `"value"` so an agent constructing a
/// Stage body has a clear, schema-discoverable contract for
/// which payload shape applies.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum OutputTop {
    /// Keep the top `value` posts after threshold filtering.
    /// Acts as an absolute cap; if the surviving bucket is
    /// smaller than `value`, everything passes through.
    Fixed(u64),
    /// Keep the top `ceil(N · value)` posts. `value` is in
    /// `[0.0, 1.0]` — e.g. `0.25` = top quarter.
    Fraction(f64),
}

fn default_true() -> bool { true }

impl Stage {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(t) = self.output_threshold {
            if !t.is_finite() || !(0.0..=1.0).contains(&t) {
                return Err(format!("output_threshold ({t}) must be in [0.0, 1.0]"));
            }
        }
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
