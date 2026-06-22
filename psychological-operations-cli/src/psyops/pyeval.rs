//! Run a psyop expression as Python via the in-process plugin executor.
//!
//! Each psyop expression (filter `custom`, sort `Custom`, output_top `Python`)
//! used to be Starlark evaluated in-process; now the CLI ships the operator's
//! code + its `input` to the objectiveai host's embedded Python runtime via
//! the `python` command and reads back the JSON the script produced (its
//! trailing expression value, else captured stdout).

use objectiveai_sdk::cli::command::python::{self, Path, Request};

use crate::error::Error;

/// Execute `code` with `input` exposed as the Python global `input`; return
/// the script's output as JSON.
pub async fn run(
    ctx: &crate::context::Context,
    code: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let req = Request {
        path_type: Path::Python,
        code: code.to_string(),
        input: Some(input),
        base: Default::default(),
    };
    python::execute(&*ctx.executor, req, None)
        .await
        .map_err(|e| Error::ObjectiveAiCli(format!("python: {e}")))
}
