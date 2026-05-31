use objectiveai_sdk::cli::output::Level;

use psychological_operations_cli::emit::{emit_error, emit_notification_from_payload};

#[tokio::main]
async fn main() {
    let cfg = psychological_operations_cli::load_config();
    let args = std::env::args_os();

    match psychological_operations_cli::run(args, &cfg).await {
        Ok(output) => {
            if !output.is_empty() {
                emit_notification_from_payload(&output);
            }
        }
        Err(e) => {
            emit_error(Level::Error, true, serde_json::Value::String(e));
            std::process::exit(1);
        }
    }
}
