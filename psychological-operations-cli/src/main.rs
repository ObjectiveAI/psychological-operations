use objectiveai_cli_sdk::output::Level;

use psychological_operations_cli::emit::{emit_error, emit_notification_from_payload};

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
                emit_error(Level::Error, true, serde_json::Value::String(e));
            }
            std::process::exit(1);
        }
    }
}
