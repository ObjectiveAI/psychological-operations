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
            if !output.is_empty() {
                println!("{output}");
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
