//! `targets schema` arm — emit the JSON Schema for a
//! [`Destination`], so agents / operators can see what shape
//! `targets add <selector> '<json>'` accepts.

use psychological_operations_sdk::cli::Output;

use crate::error::Error;
use crate::targets::destinations::Destination;

pub(super) fn run() -> Result<Output, Error> {
    Ok(Output::Schema(schemars::schema_for!(Destination)))
}
