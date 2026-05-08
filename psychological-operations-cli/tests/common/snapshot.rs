//! Snapshot assertion modeled on
//! `objectiveai-api/tests/common/stream_harness.rs::assert_snapshot`.
//!
//! Each test compares actual stdout / stderr against a committed
//! file under `assets/<test_name>/{stdout,stderr}.txt`.
//! Set `UPDATE_PSYOPS_SNAPSHOTS=1` to regenerate.

const SNAPSHOT_ENV: &str = "UPDATE_PSYOPS_SNAPSHOTS";

/// Strip non-deterministic substrings from CLI output before
/// snapshotting. Tests should call this on stdout / stderr
/// before passing them to `assert_snapshot`.
///
/// Currently scrubs:
///   - `Logs ID: fnexec-<hex>-<digits>` → `Logs ID: <id>`
///     (objectiveai's per-execution log id contains a wall-clock
///     timestamp suffix that varies across runs.)
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("Logs ID: ") {
            // Replace fnexec-<hex>-<digits> with a placeholder.
            // A raw substring scrub is enough — we don't need a
            // real regex dep for one pattern.
            let placeholder = scrub_logs_id(rest);
            out.push_str("Logs ID: ");
            out.push_str(&placeholder);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !s.ends_with('\n') {
        out.pop();
    }
    out
}

fn scrub_logs_id(rest: &str) -> String {
    // rest looks like "fnexec-fe70425df6a34dcc91808cea249d7496-1778269912"
    // Replace anything matching that shape with "<id>".
    if let Some(after_prefix) = rest.strip_prefix("fnexec-") {
        // Split on '-'; if we have exactly two segments and both
        // are hex/digits, replace.
        if let Some(dash) = after_prefix.find('-') {
            let (hash, ts) = after_prefix.split_at(dash);
            let ts = &ts[1..];
            if hash.chars().all(|c| c.is_ascii_hexdigit())
                && ts.chars().all(|c| c.is_ascii_digit())
            {
                return "<id>".to_string();
            }
        }
    }
    rest.to_string()
}

/// Compare `actual` against `expected_static` (from `include_str!`),
/// or write `actual` to `path` when `UPDATE_PSYOPS_SNAPSHOTS=1`.
///
/// `path` is the absolute path to the snapshot file (typically
/// built via `concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets/...")`)
/// so update-mode can open and rewrite it.
pub fn assert_snapshot(actual: &str, path: &str, expected_static: &str) {
    if std::env::var(SNAPSHOT_ENV).as_deref() == Ok("1") {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).expect("create snapshot parent dir");
        }
        std::fs::write(path, actual).expect("write snapshot");
        eprintln!("Updated snapshot: {path}");
        let written = std::fs::read_to_string(path).expect("re-read snapshot");
        assert_eq!(actual, written.trim_end_matches('\n'));
    } else {
        // Strip CR so a snapshot file that got auto-CRLFed by git
        // on Windows still compares equal to the LF-only stdout
        // we capture from the CLI subprocess.
        let expected = expected_static.replace('\r', "");
        assert_eq!(
            actual,
            expected.trim_end_matches('\n'),
            "snapshot mismatch at {path}: re-run with {SNAPSHOT_ENV}=1",
        );
    }
}
