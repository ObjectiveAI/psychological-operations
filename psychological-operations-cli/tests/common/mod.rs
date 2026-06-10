//! Integration-test harness. Each test:
//!   1. Constructs a `TestEnv`. The constructor copies the
//!      committed initial state from
//!      `assets/<name>/.objectiveai/` (crate-root `assets/`, NOT
//!      under `tests/`) to the runtime CONFIG_BASE_DIR
//!      `tests/.t-<name>/.objectiveai/`. Then it manually copies
//!      our built binary to
//!      `<base>/plugins/psychological-operations/plugin[.exe]` so
//!      the objectiveai host can dispatch to it.
//!   2. Spawns the `objectiveai` host binary with
//!      `psychological-operations <subcmd> …` args — exercising
//!      the real `objectiveai psychological-operations <subcmd>`
//!      dispatch path, not our binary directly.
//!   3. Captures stdout + stderr.
//!   4. Asserts against committed snapshots under
//!      `assets/<name>/{stdout,stderr}.txt`. The host wraps our
//!      plugin's PluginOutput stream with `{"type":"begin"}` /
//!      `{"type":"end"}` bookend lines on stdout.
//!
//! Each test asset folder is laid out:
//!   assets/<name>/
//!   ├── .objectiveai/                                   # initial state (committed)
//!   │   └── plugins/psychological-operations/...        # our state lives here
//!   ├── stdout.txt                                      # expected stdout
//!   └── stderr.txt                                      # expected stderr
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
fn target_binaries_dir() -> PathBuf { tests_dir().join(".target-binaries") }

/// Run `psychological-operations-browser/scripts/build-bundle.{ps1,sh}`
/// once per cargo-test process so the CLI's build.rs finds the
/// embedded browser bundle. Idempotent — the script overwrites
/// the same paths each time.
fn ensure_browser_bundle() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let target = host_triple();
        let status = if cfg!(windows) {
            // Prefer pwsh (PowerShell 7); fall back to Windows PowerShell.
            let shell = if Command::new("pwsh").arg("-NoProfile").arg("-Command").arg("$null").status().map(|s| s.success()).unwrap_or(false) {
                "pwsh"
            } else {
                "powershell"
            };
            Command::new(shell)
                .args(["-NoProfile", "-File", "psychological-operations-browser/scripts/build-bundle.ps1"])
                .arg("-Release")
                .arg("-Target").arg(target)
                .current_dir(repo_root())
                .status()
        } else {
            Command::new(bash_command())
                .arg("psychological-operations-browser/scripts/build-bundle.sh")
                .arg("--release")
                .arg("--target").arg(target)
                .current_dir(repo_root())
                .status()
        }
        .expect("spawn build-bundle script");
        assert!(status.success(), "psychological-operations-browser build-bundle failed");
    });
}


/// Build our `psychological-operations` binary once per cargo-test
/// process. Subsequent calls return the cached path.
pub fn psyops_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        ensure_browser_bundle();
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

/// Version of the objectiveai host binary the test harness downloads.
/// Bump this when you want tests to run against a newer release
/// (snapshots are wire-format-coupled to the host version, so a bump
/// often requires `UPDATE_PSYOPS_SNAPSHOTS=1` regen alongside it).
const OBJECTIVEAI_VERSION: &str = "2.0.10";

/// Filename for the prebuilt `objectiveai` release asset on the
/// current host, matching the upload convention in
/// `objectiveai/.github/workflows/release.yml`. We pull the
/// `-no-viewer` variant because the Tauri viewer is dead weight for
/// the score-path tests.
fn objectiveai_asset_name() -> &'static str {
    if      cfg!(all(target_os = "windows", target_arch = "x86_64"))  { "objectiveai-windows-x86_64-no-viewer.exe" }
    else if cfg!(all(target_os = "macos",   target_arch = "aarch64")) { "objectiveai-macos-aarch64-no-viewer" }
    else if cfg!(all(target_os = "macos",   target_arch = "x86_64"))  { "objectiveai-macos-x86_64-no-viewer" }
    else if cfg!(all(target_os = "linux",   target_arch = "aarch64")) { "objectiveai-linux-aarch64-no-viewer" }
    else if cfg!(all(target_os = "linux",   target_arch = "x86_64"))  { "objectiveai-linux-x86_64-no-viewer" }
    else { panic!("unsupported host platform — extend objectiveai_asset_name()") }
}

/// Download (once) and cache the prebuilt `objectiveai` host binary
/// from the GitHub release tagged `v<OBJECTIVEAI_VERSION>`. Subsequent
/// test-process invocations reuse the cached path.
///
/// Cache layout: `tests/.target-binaries/objectiveai-release/objectiveai-v<ver>-<asset>`.
/// The version-prefixed filename means a `OBJECTIVEAI_VERSION` bump
/// invalidates the cache automatically — no manual cleanup, no hash
/// check required.
pub fn objectiveai_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let cache_dir = target_binaries_dir().join("objectiveai-release");
        std::fs::create_dir_all(&cache_dir).expect("create objectiveai cache dir");
        let asset = objectiveai_asset_name();
        let cached = cache_dir.join(format!("objectiveai-v{OBJECTIVEAI_VERSION}-{asset}"));
        if !cached.exists() {
            let url = format!(
                "https://github.com/ObjectiveAI/objectiveai/releases/download/v{OBJECTIVEAI_VERSION}/{asset}",
            );
            eprintln!("downloading objectiveai v{OBJECTIVEAI_VERSION}: {url}");
            let bytes = reqwest::blocking::get(&url)
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.bytes())
                .unwrap_or_else(|e| panic!("download {url}: {e}"));
            std::fs::write(&cached, &bytes)
                .unwrap_or_else(|e| panic!("write {}: {e}", cached.display()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&cached)
                    .expect("downloaded binary perms").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&cached, perms)
                    .expect("chmod downloaded binary");
            }
        }
        cached
    }).as_path()
}

/// Generic per-psyop git-init: walks `psyops_dir`, and for each
/// subdirectory containing a `psyop.json` (and no existing `.git`),
/// runs the same publish flow `psyops publish` uses. Author /
/// email / commit time are pinned so the resulting commit_sha is
/// byte-stable across machines (which is what the seeded
/// `data.db` rows reference).
///
/// Asset folders just drop in whatever psyops they need under
/// `.objectiveai/plugins/psychological-operations/psyops/<name>/psyop.json`;
/// the harness handles all of them uniformly.
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
    /// initial state from `assets/<name>/.objectiveai/` if present.
    /// Then install our plugin binary into the runtime's plugins
    /// subdir, and generically git-init every psyop dir found under
    /// `psyops/` so the on-disk state matches what `psyops publish`
    /// would have produced (committed assets can't include nested
    /// `.git` dirs without git treating them as embedded repos).
    pub fn new(name: &str) -> Self {
        // Per-test runtime layout mirrors the live install:
        //
        //   <root>/.t-<name>/.objectiveai/                    ← CONFIG_BASE_DIR
        //   <root>/.t-<name>/.objectiveai/plugins/psychological-operations/
        //     ├── plugin[.exe]                                ← installed binary
        //     ├── data.db / psyops/ / config.json / ...       ← our state
        //
        // Root is the OS temp dir, not `tests/` — the workspace path
        // (~80 chars on this machine) + the layout below (~70 chars
        // including `psyops/<name>/.git/`) plus git2's own
        // sub-paths blow past Windows MAX_PATH (260). Using
        // `std::env::temp_dir()` keeps the prefix to ~30 chars on
        // Windows (`C:\Users\<user>\AppData\Local\Temp\`) which
        // leaves headroom for `.git/objects/<sha>/...` files.
        let runtime = std::env::temp_dir().join("psyops-t").join(name);
        let _ = std::fs::remove_dir_all(&runtime);
        let base = runtime.join(".objectiveai");
        let state = base.join("plugins").join("psychological-operations");
        std::fs::create_dir_all(&state).expect("create test state dir");

        // Copy the asset's .objectiveai/ verbatim into the runtime
        // CONFIG_BASE_DIR. Asset structure:
        //   assets/<name>/.objectiveai/plugins/psychological-operations/data.db
        //   assets/<name>/.objectiveai/plugins/psychological-operations/psyops/...
        let assets = assets_dir().join(name);
        let initial = assets.join(".objectiveai");
        if initial.exists() {
            copy_dir_recursive(&initial, &base);
        }

        // Manual plugin install: copy our built binary to the
        // per-plugin subdir as `plugin[.exe]`, matching the layout
        // `objectiveai plugins install` produces from GitHub. We use
        // manual copy because the install command requires network +
        // a published release.
        let plugin_bin = if cfg!(windows) { "plugin.exe" } else { "plugin" };
        std::fs::copy(psyops_binary(), state.join(plugin_bin))
            .expect("install plugin binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = state.join(plugin_bin);
            let mut perms = std::fs::metadata(&p).expect("plugin perms").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&p, perms).expect("chmod plugin");
        }

        let psyops_dir = state.join("psyops");
        if psyops_dir.exists() {
            git_init_psyops(&psyops_dir);
        }

        Self { name: name.into(), dir: state, base, assets }
    }

    /// Build a `Command` for invoking our plugin via the objectiveai
    /// host (the real dispatch path: `objectiveai psychological-operations
    /// <subcmd>`). Per-subprocess env, not per-process.
    pub fn cmd(&self) -> Command {
        let mut cmd = Command::new(objectiveai_binary());
        cmd.arg("psychological-operations");
        cmd.env("CONFIG_BASE_DIR",                              &self.base);
        // Score-path subprocesses re-invoke the objectiveai CLI for
        // `functions get` / `executions create`. Point them at the
        // same no-viewer release binary we downloaded for the host so
        // tests don't spawn viewer windows.
        cmd.env("PSYCHOLOGICAL_OPERATIONS_OBJECTIVEAI_BINARY",  objectiveai_binary());
        // X mocking moved from this process-wide env var to a
        // per-psyop `mock` field. Every test fixture's psyop.json
        // sets `"mock": true` instead.
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
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // Wipe the entire runtime dir (parent of `.objectiveai`),
        // not just `self.base` — keeps tests/ clean between runs.
        let runtime = self.base.parent().unwrap_or(&self.base).to_path_buf();
        if std::env::var_os("PSYOPS_KEEP_TEST_STATE").is_some() {
            eprintln!(
                "PSYOPS_KEEP_TEST_STATE — leaving {}",
                runtime.display(),
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&runtime);
    }
}
