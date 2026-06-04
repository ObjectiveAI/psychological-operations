use objectiveai_sdk::cli::Level;

use psychological_operations_cli::output::OutputResult;

#[tokio::main]
async fn main() {
    let cfg = psychological_operations_cli::run::load_config();
    let args = std::env::args_os();

    match psychological_operations_cli::commands::run(args, &cfg).await {
        Ok(output) => {
            if !output.is_empty() {
                let value: serde_json::Value = serde_json::from_str(&output)
                    .unwrap_or_else(|_| serde_json::Value::String(output));
                OutputResult::Notification(serde_json::json!({ "value": value })).emit();
            }
        }
        Err(e) => {
            OutputResult::error(Level::Error, /* fatal */ true, serde_json::Value::String(e))
                .emit();
            std::process::exit(1);
        }
    }
}
