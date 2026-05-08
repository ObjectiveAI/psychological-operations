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

use psychological_operations_cli::db::{Db, MediaUrl, Origin, Post};
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
const SHARED_PSYOP_COMMIT_SHA: &str = "82083cb385f9d2dc616126474601afd1e4d4050b";

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

fn fake_post(id: &str, handle: &str, text: &str) -> Post {
    Post {
        id: id.into(),
        handle: handle.into(),
        text: text.into(),
        images: Vec::<MediaUrl>::new(),
        videos: Vec::<MediaUrl>::new(),
        created: "2026-01-01T00:00:00.000Z".into(),
        likes: 100, retweets: 10, replies: 5, impressions: 1000,
    }
}

fn build_psyops_run_with_pre_hydrated_posts() {
    let asset = assets_dir().join("psyops_run_with_pre_hydrated_posts").join(".psychological-operations");
    std::fs::create_dir_all(&asset).unwrap();
    let _ = std::fs::remove_file(asset.join("data.db"));
    let cfg = cfg_for(&asset);
    let db = Db::open(&cfg).expect("open db");

    // Pre-hydrated posts: rows in posts + contents + sources for
    // psyop "test-psyop" referencing the deterministic SHA. The
    // for_you_queue stays empty so `psyops run` skips the X
    // /tweets/{id} hydration call entirely.
    for (id, handle, text) in [
        ("1910000000000000001", "alice", "first hydrated tweet"),
        ("1910000000000000002", "bob",   "second hydrated tweet"),
    ] {
        let inserted = db.insert_post(
            &fake_post(id, handle, text),
            "test-psyop",
            SHARED_PSYOP_COMMIT_SHA,
            &Origin::ForYou,
        ).expect("insert_post");
        assert!(inserted);
    }
    eprintln!("wrote seed: {}", asset.join("data.db").display());
}

/// Replays the harness's git-init in a tmp dir against the
/// already-on-disk fixture and returns the resulting commit SHA.
/// Use when a scenario's psyop.json content varies between runs
/// (so SHARED_PSYOP_COMMIT_SHA is the wrong value to hardcode).
fn fixture_commit_sha(asset: &std::path::Path, psyop_name: &str) -> String {
    let psyop_json = asset.join("psyops").join(psyop_name).join("psyop.json");
    let content = std::fs::read_to_string(&psyop_json).expect("read fixture psyop.json");
    let tmp = std::env::temp_dir().join(format!(
        "psyops-test-seed-{}-{}",
        psyop_name,
        std::process::id(),
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create tmp dir");
    let cfg = Config {
        commit_author_name:  Some("psyops-test".into()),
        commit_author_email: Some("test@psyops.invalid".into()),
        commit_time:         Some(1767225600),
        ..Default::default()
    };
    let sha = psychological_operations_cli::publish::publish_file(
        &tmp, "psyop.json", &content, "init", &cfg,
    ).expect("publish_file");
    let _ = std::fs::remove_dir_all(&tmp);
    sha
}

fn build_psyops_run_with_pre_queued_deliveries() {
    let asset = assets_dir().join("psyops_run_with_pre_queued_deliveries").join(".psychological-operations");
    std::fs::create_dir_all(&asset).unwrap();
    let _ = std::fs::remove_file(asset.join("data.db"));
    let cfg = cfg_for(&asset);
    let db = Db::open(&cfg).expect("open db");

    // The fixture's content has queries set, so the SHA differs
    // from SHARED_PSYOP_COMMIT_SHA. Compute it on the fly to match
    // whatever the harness will produce.
    let sha = fixture_commit_sha(&asset, "test-psyop");

    let target_json = r#"{"type":"stdout","mode":"urls"}"#;
    let post_ids_json = r#"["1900000000000000111","1900000000000000222"]"#;
    let _ = db.enqueue_delivery(
        "test-psyop",
        &sha,
        target_json,
        post_ids_json,
    ).expect("enqueue_delivery");
    eprintln!("wrote seed: {} (psyop sha {sha})", asset.join("data.db").display());
}

fn build_targets_deliver_drains_queue() {
    let asset = assets_dir().join("targets_deliver_drains_queue").join(".psychological-operations");
    std::fs::create_dir_all(&asset).unwrap();
    let _ = std::fs::remove_file(asset.join("data.db"));
    let cfg = cfg_for(&asset);
    let db = Db::open(&cfg).expect("open db");

    // Two pre-queued rows: one stdout-urls + one stdout-json. The
    // psyop.json fixture matches SHARED_PSYOP_COMMIT_SHA exactly
    // (no queries, just min_posts: 2 + a single mock stage), so we
    // can pin the SHA constant rather than recomputing.
    let post_ids_json = r#"["1900000000000000111","1900000000000000222"]"#;
    for target_json in [
        r#"{"type":"stdout","mode":"urls"}"#,
        r#"{"type":"stdout","mode":"json"}"#,
    ] {
        let _ = db.enqueue_delivery(
            "test-psyop",
            SHARED_PSYOP_COMMIT_SHA,
            target_json,
            post_ids_json,
        ).expect("enqueue_delivery");
    }
    eprintln!("wrote seed: {}", asset.join("data.db").display());
}

fn main() {
    let scenario = std::env::args().nth(1)
        .expect("usage: build_test_seed <scenario-name>");
    match scenario.as_str() {
        "psyops_run_with_for_you_queue"          => build_psyops_run_with_for_you_queue(),
        "psyops_run_with_pre_hydrated_posts"     => build_psyops_run_with_pre_hydrated_posts(),
        "psyops_run_with_pre_queued_deliveries"  => build_psyops_run_with_pre_queued_deliveries(),
        "targets_deliver_drains_queue"           => build_targets_deliver_drains_queue(),
        other => panic!("unknown scenario: {other}"),
    }
}
