#[tokio::main]
async fn main() {
    let cfg = psychological_operations_cli::run::load_config();
    let args = std::env::args_os();
    let ok = psychological_operations_cli::commands::run(args, &cfg).await;
    if !ok {
        std::process::exit(1);
    }
}
