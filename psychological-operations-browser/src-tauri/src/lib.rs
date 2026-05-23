mod args;
mod stdio;

use clap::Parser;
use psychological_operations_browser_sdk::output::Output;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args = match args::Args::try_parse() {
        Ok(a) => a,
        Err(e) => {
            // Clap would normally render to stderr; we route everything
            // through JSONL on stdout instead.
            let _ = stdio::write_output(&Output::Log {
                message: format!("args: {e}"),
            });
            std::process::exit(e.exit_code());
        }
    };
    let mode = match args.mode() {
        Ok(m) => m,
        Err(e) => {
            let _ = stdio::write_output(&Output::Log {
                message: format!("args: {e}"),
            });
            std::process::exit(2);
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(args)
        .manage(mode)
        .invoke_handler(tauri::generate_handler![stdio::stdio_respond])
        .setup(|app| {
            stdio::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
