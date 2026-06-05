pub use psychological_operations_sdk::cli::destinations::stdout::Stdout;

use crate::events::Event;
use super::{json_body, Subject};

pub async fn send(_cfg: &Stdout, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    let body = json_body::build(subject);
    crate::output::OutputResult::from(Event::TargetDelivered {
        body: serde_json::to_value(&body)?,
    })
    .emit();
    Ok(())
}
