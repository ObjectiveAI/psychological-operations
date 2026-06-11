//! Snapshot assertion modeled on
//! `objectiveai-api/tests/common/stream_harness.rs::assert_snapshot`.
//!
//! Each test compares actual stdout / stderr against a committed
//! file under `assets/<test_name>/{stdout,stderr}.txt`.
//! Set `UPDATE_PSYOPS_SNAPSHOTS=1` to regenerate.

const SNAPSHOT_ENV: &str = "UPDATE_PSYOPS_SNAPSHOTS";

/// Strip non-deterministic substrings from CLI output before
/// snapshotting. Tests should call this on stdout / stderr
/// before passing them to `assert_snapshot` when their output
/// includes `objectiveai functions executions` log ids.
///
/// Currently scrubs:
///   - `Logs ID: fnexec-<hex>-<digits>` → `Logs ID: <id>`
///   - `"fnexec-<hex>-<digits>"` (e.g. inside the
///     `log_stream_ready` notification value forwarded from the
///     `objectiveai functions executions create` subprocess) → `"<id>"`.
///
/// objectiveai's per-execution log id contains a wall-clock
/// timestamp suffix that varies across runs.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("Logs ID: ") {
            let placeholder = scrub_logs_id(rest);
            out.push_str("Logs ID: ");
            out.push_str(&placeholder);
        } else {
            out.push_str(&scrub_fnexec_in_line(line));
        }
        out.push('\n');
    }
    if !s.ends_with('\n') {
        out.pop();
    }
    out
}

/// Replace every occurrence of `fnexec-<hex>-<digits>` inside `line`
/// with the literal placeholder `<id>`. Cheap state-machine scan — no
/// regex dep.
fn scrub_fnexec_in_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let bytes = line.as_bytes();
    while i < bytes.len() {
        if bytes[i..].starts_with(b"fnexec-") {
            // Look for the matching tail: <hex chars>-<digits>.
            let after_prefix = i + b"fnexec-".len();
            let hash_end = bytes[after_prefix..].iter()
                .position(|&b| !b.is_ascii_hexdigit())
                .map(|n| after_prefix + n)
                .unwrap_or(bytes.len());
            if hash_end < bytes.len() && bytes[hash_end] == b'-' {
                let ts_start = hash_end + 1;
                let ts_end = bytes[ts_start..].iter()
                    .position(|&b| !b.is_ascii_digit())
                    .map(|n| ts_start + n)
                    .unwrap_or(bytes.len());
                if ts_end > ts_start && hash_end > after_prefix {
                    out.push_str("<id>");
                    i = ts_end;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
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
    // No host-preamble stripping anymore: the 2.0.x auto-updater
    // pre-dispatch chatter (and the `{"type":"begin"}` marker the
    // old strip keyed on) are both gone at host v2.1.1 — `update`
    // is an explicit command now and plugin output flows through
    // unframed.
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
