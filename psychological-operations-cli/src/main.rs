#[tokio::main]
async fn main() {
    let ctx = match psychological_operations_cli::context::Context::new().await {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("fatal: {e}");
            std::process::exit(1);
        }
    };
    let args = std::env::args_os();
    let ok = psychological_operations_cli::commands::run(args, &ctx).await;
    // Hard exit on BOTH paths. The PluginExecutor's stdin listener
    // parks a blocking read on the runtime's blocking pool, and the
    // objectiveai host (2.1.1) deliberately keeps our stdin open
    // until it sees our stdout EOF — so returning from main (whose
    // runtime drop waits out blocking tasks) deadlocks: the host
    // waits on us, we wait on the host. `process::exit` skips
    // runtime shutdown; every output line is already flushed (std
    // stdout is line-buffered, the executor flushes per write).
    std::process::exit(if ok { 0 } else { 1 });
}
