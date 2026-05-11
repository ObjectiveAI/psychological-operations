use objectiveai_cli_lib::output::{Error as PluginError, Level};
use objectiveai_cli_lib::plugin_output::PluginOutput;

#[tokio::main]
async fn main() {
    let cfg = psychological_operations_cli::load_config();
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();

    // Chromium spawns the native-messaging host with a single CLI
    // arg `--parent-window=<HWND>` (Windows) plus the manifest path
    // and origin URL on macOS/Linux. Detect that signature and route
    // to native-host mode automatically — this lets us point the NM
    // manifest at the main psychological-operations.exe directly
    // instead of via a .cmd wrapper (the wrapper mangles binary
    // stdin under cmd.exe, breaking the NM length-prefix protocol).
    //
    // Native-host mode uses Chromium's framed binary protocol on
    // stdio, NOT our plugin's PluginOutput JSONL format — so it
    // bypasses the JSONL emit path entirely.
    let invoked_as_native_host = args.iter().skip(1).any(|a| {
        a.to_string_lossy().starts_with("--parent-window")
            || a.to_string_lossy().starts_with("chrome-extension://")
    });
    let synthesized_args: Vec<std::ffi::OsString> = if invoked_as_native_host {
        vec![args[0].clone(), std::ffi::OsString::from("native-host")]
    } else {
        args
    };

    match psychological_operations_cli::run(synthesized_args.into_iter(), &cfg).await {
        Ok(output) => {
            if invoked_as_native_host {
                // NM-host wrote framed JSON directly; nothing to wrap.
                return;
            }
            if !output.is_empty() {
                emit_notification_from_payload(&output);
            }
        }
        Err(e) => {
            if invoked_as_native_host {
                // NM-host context: emit on stderr the host (Chromium)
                // can show; can't emit JSONL because the protocol's
                // binary-framed on stdout.
                eprintln!("error: {e}");
            } else {
                emit_error(Level::Error, true, e);
            }
            std::process::exit(1);
        }
    }
}

/// Wrap a command's result payload in a PluginOutput::Notification
/// and write it to stdout as one JSON line.
///
/// The payload arrives as the `Display` form of our `Output` enum —
/// for `Output::Api(s)` / `Output::ConfigGet(s)` the inner string is
/// already valid JSON (every handler that returns these emits JSON).
/// `Output::ConfigSet` is the literal text `"ok"`. Wrap the parsed
/// value if we can; otherwise wrap the string verbatim.
fn emit_notification_from_payload(payload: &str) {
    let value: serde_json::Value = serde_json::from_str(payload)
        .unwrap_or_else(|_| serde_json::Value::String(payload.to_string()));
    // PluginOutput::Notification(value) uses serde's internal tagging,
    // which requires the value to serialize as a JSON object so the
    // `"type":"notification"` discriminator can be merged in. Arrays /
    // strings / numbers can't carry a tag, so always wrap under `value`.
    let wrapped = serde_json::json!({ "value": value });
    let out = PluginOutput::Notification(wrapped);
    let line = serde_json::to_string(&out)
        .expect("PluginOutput serializes");
    println!("{line}");
}

fn emit_error(level: Level, fatal: bool, message: String) {
    let out = PluginOutput::Error(PluginError { level, fatal, message });
    let line = serde_json::to_string(&out)
        .expect("PluginOutput serializes");
    println!("{line}");
}
