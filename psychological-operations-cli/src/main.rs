#[tokio::main]
async fn main() {
    let cfg = psychological_operations_cli::load_config();
    match psychological_operations_cli::run(std::env::args_os(), &cfg).await {
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
