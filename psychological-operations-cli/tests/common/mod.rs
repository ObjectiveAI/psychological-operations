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

/// Run `psychological-operations-chrome/build.sh` once per
/// cargo-test process to ensure the embedded chrome bundle is
/// present before we build our binary. Idempotent — the script
/// fingerprint-short-circuits when the embed dir is fresh.
fn ensure_chrome_bundle() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        // Pass a relative path: bash on Windows (Git Bash)
        // mangles backslashes if you give it a Windows-absolute
        // path. cwd-relative + posix-style separators sidesteps
        // that.
        let status = Command::new("bash")
            .arg("psychological-operations-chrome/build.sh")
            .arg("--release")
            .current_dir(repo_root())
            .status()
            .expect("spawn bash psychological-operations-chrome/build.sh \
                     (Windows: requires Git Bash on PATH)");
        assert!(status.success(), "psychological-operations-chrome build failed");
    });
}

/// Build our `psychological-operations` binary once per cargo-test
/// process. Subsequent calls return the cached path.
pub fn psyops_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        ensure_chrome_bundle();
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

/// Build `objectiveai-cli` once per cargo-test process.
/// `viewer` feature disabled — viewer pulls in ratatui and is
/// unrelated to the score path.
pub fn objectiveai_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
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
                "--features", "rustpython,systempython,claude-agent-sdk,updater",
                "--release",
                "--target-dir", target.to_str().unwrap(),
            ])
            .status()
            .expect("spawn cargo build objectiveai-cli");
        assert!(status.success(), "objectiveai-cli build failed");
        let exe = if cfg!(windows) { "objectiveai.exe" } else { "objectiveai" };
        target.join("release").join(exe)
    }).as_path()
}

fn ensure_objectiveai_state_dir() -> PathBuf {
    let dir = objectiveai_state_dir();
    std::fs::create_dir_all(&dir).expect("create .objectiveai state dir");
    dir
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
    /// if present. Tests with no initial state can omit that
    /// directory; the runtime dir starts empty.
    pub fn new(name: &str) -> Self {
        let _ = ensure_objectiveai_state_dir();
        let dir = tests_dir().join(format!(".psychological-operations-{name}"));
        let _ = std::fs::remove_dir_all(&dir);

        let assets = assets_dir().join(name);
        let initial = assets.join(".psychological-operations");
        if initial.exists() {
            copy_dir_recursive(&initial, &dir);
        } else {
            std::fs::create_dir_all(&dir).expect("create test dir");
        }

        Self { name: name.into(), dir, assets }
    }

    /// Build a `Command` for our CLI with the right env vars set
    /// (per-subprocess, not per-process). Includes pinned
    /// commit-author + commit-time so any `psyops publish`
    /// invocations produce byte-stable commit SHAs.
    pub fn cmd(&self) -> Command {
        let mut cmd = Command::new(psyops_binary());
        cmd.env("PSYCHOLOGICAL_OPERATIONS_BASE_DIR",            &self.dir);
        cmd.env("PSYCHOLOGICAL_OPERATIONS_MOCK_X_API",          "true");
        cmd.env("PSYCHOLOGICAL_OPERATIONS_OBJECTIVEAI_BINARY",  objectiveai_binary());
        cmd.env("CONFIG_BASE_DIR",                              objectiveai_state_dir());
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
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
