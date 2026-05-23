#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

use psychological_operations_browser::args::Args;

fn main() {
    let args = Args::parse();
    if let Err(e) = psychological_operations_browser::run(args) {
        eprintln!("ERROR: {e:#}");
        std::process::exit(1);
    }
}
