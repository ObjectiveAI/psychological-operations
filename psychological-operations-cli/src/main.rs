#[tokio::main]
async fn main() {
    let ctx = psychological_operations_cli::context::Context::new();
    let args = std::env::args_os();
    let ok = psychological_operations_cli::commands::run(args, &ctx).await;
    if !ok {
        std::process::exit(1);
    }
}
