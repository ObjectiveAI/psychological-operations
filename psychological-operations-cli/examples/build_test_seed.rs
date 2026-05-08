//! One-off seed-DB builder for integration tests.
//!
//! Tests must NOT execute SQL or call DB methods directly — but
//! the committed `data.db` files under `assets/<name>/.psychological-operations/`
//! have to be built somehow. This binary is that "somehow":
//! the author runs it to (re)generate the seed for a named
//! scenario, then commits the resulting `data.db`.
//!
//! Usage:
//!   cargo run -p psychological-operations-cli --example build_test_seed -- <scenario-name>
//!
//! Writes to `assets/<scenario-name>/.psychological-operations/data.db`.
//! Hardcoded scenarios live below — extend as new tests need
//! seeded state.

use std::path::PathBuf;

use psychological_operations_cli::db::Db;
use psychological_operations_cli::run::Config;

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn cfg_for(asset_dir: &std::path::Path) -> Config {
    Config {
        base_dir: Some(asset_dir.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

/// SHA the harness's git-init produces for the standard mock
/// psyop.json content + pinned author/email/time. Same content =
/// same SHA, regardless of psyop name. (Differs from what the
/// CLI's `psyops publish` would produce because the CLI re-
/// serializes via to_string_pretty; the harness commits the raw
/// file content as-is.)
const SHARED_PSYOP_COMMIT_SHA: &str = "36bc2cc006d26d683363d2b0992c876258ae99b4";

fn build_psyops_run_with_for_you_queue() {
    // Only touch the seed DB file — leave psyops/, config.json,
    // etc. intact (they're committed alongside the seed).
    let asset = assets_dir().join("psyops_run_with_for_you_queue").join(".psychological-operations");
    std::fs::create_dir_all(&asset).unwrap();
    let _ = std::fs::remove_file(asset.join("data.db"));
    let cfg = cfg_for(&asset);
    let db = Db::open(&cfg).expect("open db");
    for id in ["1900000000000000001", "1900000000000000002"] {
        let inserted = db.enqueue_for_you(id, "test-psyop", SHARED_PSYOP_COMMIT_SHA)
            .expect("enqueue");
        assert!(inserted);
    }
    eprintln!("wrote seed: {}", asset.join("data.db").display());
}

fn main() {
    let scenario = std::env::args().nth(1)
        .expect("usage: build_test_seed <scenario-name>");
    match scenario.as_str() {
        "psyops_run_with_for_you_queue" => build_psyops_run_with_for_you_queue(),
        other => panic!("unknown scenario: {other}"),
    }
}
