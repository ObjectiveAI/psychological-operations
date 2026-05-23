mod args;
mod stdio;
mod webview;

use clap::Parser;
use clap::error::ErrorKind;
use psychological_operations_browser_sdk::output::Output;

/// `--help`, `--version`, and the special
/// `DisplayHelpOnMissingArgumentOrSubcommand` case are clap's three
/// "informational" error kinds — they're not real errors, they're
/// success-with-text. Mirror the convention used in
/// `psychological-operations-cli/src/run.rs::is_informational`.
fn is_informational(e: &clap::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args = match args::Args::try_parse() {
        Ok(a) => a,
        Err(e) if is_informational(&e) => {
            let _ = Output::Help { text: e.to_string() }.emit();
            std::process::exit(0);
        }
        Err(e) => {
            let _ = Output::Error { error: e.to_string() }.emit();
            std::process::exit(e.exit_code());
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(args)
        .invoke_handler(tauri::generate_handler![stdio::report_url])
        .setup(|app| {
            stdio::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
