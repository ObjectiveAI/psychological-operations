//! Integration-test harness. Each test:
//!   1. Constructs a `TestEnv`. The constructor copies the
//!      committed initial state from
//!      `assets/<name>/.psychological-operations/` (crate-root
//!      `assets/`, NOT under `tests/`) to the runtime location
//!      `tests/.psychological-operations-<name>/`. Mutations
//!      land on the copy.
//!   2. Spawns our `psychological-operations` binary as a
//!      subprocess via `TestEnv::run` with per-call env vars.
//!   3. Captures stdout + stderr.
//!   4. Asserts against committed snapshots under
//!      `assets/<name>/{stdout,stderr}.txt`.
//!
//! Each test asset folder is laid out:
//!   assets/<name>/
//!   ├── .psychological-operations/   # initial state (committed)
//!   ├── stdout.txt                   # expected stdout
//!   └── stderr.txt                   # expected stderr
//!
//! Tests run in PARALLEL — env vars are per-subprocess
//! (`Command::env`), never set on the test process itself.
//! Drop wipes the runtime copy on completion (or
//! `PSYOPS_KEEP_TEST_STATE=1` to preserve for debugging).

#![allow(dead_code)]   // Helpers used by individual test files.

pub mod snapshot;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn manifest_dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }
fn repo_root() -> PathBuf { manifest_dir().join("..") }
fn tests_dir() -> PathBuf { manifest_dir().join("tests") }
fn assets_dir() -> PathBuf { manifest_dir().join("assets") }
fn objectiveai_state_dir() -> PathBuf { tests_dir().join(".objectiveai") }
fn target_binaries_dir() -> PathBuf { tests_dir().join(".target-binaries") }

/// Run `psychological-operations-chromium/build.sh` once per
/// cargo-test process to ensure the embedded chrome bundle is
/// present before we build our binary. Idempotent — the script
/// fingerprint-short-circuits when the embed dir is fresh.
fn ensure_chromium_bundle() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        // Use Git Bash on Windows (see bash_command rationale).
        // Pin --target so fingerprint.sh doesn't have to call
        // `rustc -vV` to detect the host.
        let status = Command::new(bash_command())
            .arg("psychological-operations-chromium/build.sh")
            .arg("--target").arg(host_triple())
            .arg("--release")
            .current_dir(repo_root())
            .status()
            .expect("spawn bash psychological-operations-chromium/build.sh");
        assert!(status.success(), "psychological-operations-chromium build failed");
    });
}

/// Build our `psychological-operations` binary once per cargo-test
/// process. Subsequent calls return the cached path.
pub fn psyops_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        ensure_chromium_bundle();
        let target = target_binaries_dir().join("psyops");
        std::fs::create_dir_all(&target).expect("create psyops target dir");
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--bin", "psychological-operations",
                "--release",
                "--target-dir", target.to_str().unwrap(),
                "--manifest-path", manifest_dir().join("Cargo.toml").to_str().unwrap(),
            ])
            .status()
            .expect("spawn cargo build psychological-operations");
        assert!(status.success(), "psychological-operations build failed");
        let exe = if cfg!(windows) { "psychological-operations.exe" } else { "psychological-operations" };
        target.join("release").join(exe)
    }).as_path()
}

/// Host triple the test process is built for. Used to pin the
/// --target arg on bundle builds so the fingerprint script
/// doesn't need to invoke `rustc -vV` (which fails when bash's
/// PATH doesn't have rustc — common in WSL-bash subprocesses).
fn host_triple() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64"))    { "x86_64-pc-windows-msvc" }
    else if cfg!(all(target_os = "macos",   target_arch = "aarch64")) { "aarch64-apple-darwin" }
    else if cfg!(all(target_os = "macos",   target_arch = "x86_64"))  { "x86_64-apple-darwin" }
    else if cfg!(all(target_os = "linux",   target_arch = "aarch64")) { "aarch64-unknown-linux-gnu" }
    else if cfg!(all(target_os = "linux",   target_arch = "x86_64"))  { "x86_64-unknown-linux-gnu" }
    else { panic!("unsupported host triple — extend host_triple()") }
}

/// Path to bash. On Windows, prefer Git Bash over WSL bash:
/// WSL mangles Windows paths (rewrites `C:\...` to `/mnt/c/...`),
/// and its rustc / cargo PATH usually doesn't include the host's
/// Rust installation — both blow up the bundle build scripts.
fn bash_command() -> &'static Path {
    static BASH: OnceLock<PathBuf> = OnceLock::new();
    BASH.get_or_init(|| {
        if cfg!(windows) {
            for candidate in [
                r"C:\Program Files\Git\bin\bash.exe",
                r"C:\Program Files (x86)\Git\bin\bash.exe",
            ] {
                let p = PathBuf::from(candidate);
                if p.exists() { return p; }
            }
        }
        PathBuf::from("bash")
    }).as_path()
}

/// Build a sister-bundle that `objectiveai-api`'s `build.rs`
/// validates before compiling. `script` is relative to the
/// objectiveai workspace root. The build scripts are idempotent
/// — fingerprint-short-circuit when source hashes haven't
/// changed, so repeat invocations are near-instant.
fn ensure_objectiveai_bundle(script_rel: &str, args: &[&str]) {
    let oai_root = repo_root().join("objectiveai");
    let status = Command::new(bash_command())
        .arg(script_rel)
        .args(args)
        .current_dir(&oai_root)
        .status()
        .expect("spawn bash <bundle build.sh>");
    assert!(
        status.success(),
        "objectiveai bundle build failed: bash {script_rel} {args:?}",
    );
}

/// Build `objectiveai-cli` once per cargo-test process.
/// `viewer` feature disabled — viewer pulls in ratatui and is
/// unrelated to the score path. Cargo's incremental build means
/// the second + later cargo-test runs only do a fingerprint
/// check (sub-second) when nothing changed.
pub fn objectiveai_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        // Pre-built sister bundles required by objectiveai-api's
        // build.rs. Both fingerprint-short-circuit on subsequent
        // calls.
        ensure_objectiveai_bundle(
            "objectiveai-mcp-filesystem/build.sh",
            // Filesystem MCP is always linux-musl regardless of
            // the host (Docker injection target).
            &["--target", "x86_64-unknown-linux-musl", "--release"],
        );
        ensure_objectiveai_bundle(
            "objectiveai-claude-agent-sdk-runner/build.sh",
            // Built for the test host; pin --target so
            // fingerprint.sh doesn't need rustc on PATH.
            &["--target", host_triple(), "--release"],
        );

        let target = target_binaries_dir().join("objectiveai");
        std::fs::create_dir_all(&target).expect("create objectiveai target dir");
        let manifest = repo_root()
            .join("objectiveai")
            .join("objectiveai-cli")
            .join("Cargo.toml");
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--manifest-path", manifest.to_str().unwrap(),
                "--no-default-features",
                // `updater` dropped: phones home to GitHub on every
                // invocation — non-deterministic + slow + leaks
                // network errors into stderr snapshots. `viewer`
                // also dropped (ratatui dep, unused by score path).
                "--features", "rustpython,systempython,claude-agent-sdk",
                "--release",
                "--target-dir", target.to_str().unwrap(),
            ])
            .status()
            .expect("spawn cargo build objectiveai-cli");
        assert!(status.success(), "objectiveai-cli build failed");
        let exe = if cfg!(windows) { "objectiveai-cli.exe" } else { "objectiveai-cli" };
        target.join("release").join(exe)
    }).as_path()
}

fn ensure_objectiveai_state_dir() -> PathBuf {
    let dir = objectiveai_state_dir();
    std::fs::create_dir_all(&dir).expect("create .objectiveai state dir");
    dir
}

/// Generic per-psyop git-init: walks `psyops_dir`, and for each
/// subdirectory containing a `psyop.json` (and no existing `.git`),
/// runs the same publish flow `psyops publish` uses. Author /
/// email / commit time are pinned so the resulting commit_sha is
/// byte-stable across machines (which is what the seeded
/// `data.db` rows reference).
///
/// Asset folders just drop in whatever psyops they need under
/// `.psychological-operations/psyops/<name>/psyop.json`; the
/// harness handles all of them uniformly.
fn git_init_psyops(psyops_dir: &Path) {
    let cfg = psychological_operations_cli::run::Config {
        commit_author_name:  Some("psyops-test".into()),
        commit_author_email: Some("test@psyops.invalid".into()),
        commit_time:         Some(1767225600),
        ..Default::default()
    };
    for entry in std::fs::read_dir(psyops_dir).expect("read psyops dir") {
        let entry = entry.expect("psyops dir entry");
        let path = entry.path();
        if !path.is_dir() { continue; }
        let psyop_json = path.join("psyop.json");
        if !psyop_json.exists() { continue; }
        if path.join(".git").exists() { continue; }

        let content = std::fs::read_to_string(&psyop_json)
            .expect("read psyop.json");
        psychological_operations_cli::publish::publish_file(
            &path, "psyop.json", &content, "init", &cfg,
        ).expect("git-init psyop");
    }
}

/// Recursively copy `src` into `dst`. Both must exist; entries
/// in `src` are merged into `dst` (overwriting on conflict).
fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst dir");
    for entry in std::fs::read_dir(src).expect("read src dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            std::fs::copy(&from, &to)
                .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", from.display(), to.display()));
        }
    }
}

pub struct TestEnv {
    #[allow(dead_code)]
    pub base:   PathBuf,   // CONFIG_BASE_DIR for this test (gitignored)
    pub name:   String,
    pub dir:    PathBuf,   // runtime per-test base dir (gitignored)
    pub assets: PathBuf,   // tests/assets/<name>/ (committed)
}

pub struct CapturedOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CapturedOutput {
    pub fn stdout_trimmed(&self) -> &str { self.stdout.trim_end_matches('\n') }
    pub fn stderr_trimmed(&self) -> &str { self.stderr.trim_end_matches('\n') }
}

impl TestEnv {
    /// Pre-wipe the runtime per-test dir, then copy the committed
    /// initial state from `assets/<name>/.psychological-operations/`
    /// if present. After copy, generically git-init every psyop
    /// dir found under `psyops/` so the on-disk state matches what
    /// `psyops publish` would have produced (committed assets
    /// can't include nested .git dirs without git treating them
    /// as embedded repos).
    pub fn new(name: &str) -> Self {
        let _ = ensure_objectiveai_state_dir();
        // base_dir = the per-test CONFIG_BASE_DIR (acts as
        // objectiveai's base too). Our state nests inside as
        // `<base>/plugins/.psychological-operations/` per the
        // plugin-conversion layout — matching Config::base_dir().
        // Keep this prefix tight — Windows MAX_PATH (260) bites when
        // git2 creates .git/ deep under
        // `<base>/plugins/.psychological-operations/psyops/<name>/.git/...`.
        let base = tests_dir().join(format!(".t-{name}"));
        let state = base.join("plugins").join(".psychological-operations");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&state).expect("create test state dir");

        let assets = assets_dir().join(name);
        let initial = assets.join(".psychological-operations");
        if initial.exists() {
            copy_dir_recursive(&initial, &state);
        }

        let psyops_dir = state.join("psyops");
        if psyops_dir.exists() {
            git_init_psyops(&psyops_dir);
        }

        Self { name: name.into(), dir: state, base, assets }
    }

    /// Build a `Command` for our CLI with the right env vars set
    /// (per-subprocess, not per-process). Includes pinned
    /// commit-author + commit-time so any `psyops publish`
    /// invocations produce byte-stable commit SHAs.
    pub fn cmd(&self) -> Command {
        let mut cmd = Command::new(psyops_binary());
        cmd.env("CONFIG_BASE_DIR",                              &self.base);
        cmd.env("PSYCHOLOGICAL_OPERATIONS_MOCK_X_API",          "true");
        cmd.env("PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_NAME",  "psyops-test");
        cmd.env("PSYCHOLOGICAL_OPERATIONS_COMMIT_AUTHOR_EMAIL", "test@psyops.invalid");
        // Fixed epoch (2026-01-01 00:00:00 UTC). Combined with the
        // pinned author, gives every test's `psyops publish` a
        // byte-stable commit_sha across machines.
        cmd.env("PSYCHOLOGICAL_OPERATIONS_COMMIT_TIME",         "1767225600");
        cmd
    }

    /// Run a CLI invocation; capture stdout + stderr.
    pub fn run(&self, args: &[&str]) -> CapturedOutput {
        let out = self.cmd().args(args).output()
            .expect("spawn psychological-operations");
        CapturedOutput {
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Path to the per-test sqlite DB.
    pub fn db_path(&self) -> PathBuf { self.dir.join("data.db") }

    /// Read-only sqlite handle for assertions.
    pub fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.db_path()).expect("open test db")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        if std::env::var_os("PSYOPS_KEEP_TEST_STATE").is_some() {
            eprintln!(
                "PSYOPS_KEEP_TEST_STATE — leaving {}",
                self.dir.display(),
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
