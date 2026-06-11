//! Integration-test harness. Each test:
//!   1. Constructs a `TestEnv`. The constructor copies the
//!      committed initial state from
//!      `assets/<name>/.objectiveai/` (crate-root `assets/`, NOT
//!      under `tests/`) to a runtime CONFIG_BASE_DIR in the OS
//!      temp dir. Then it manually installs our built binary at
//!      `<base>/plugins/<owner>/psychological-operations/<version>/plugin[.exe]`
//!      — the v2.1.1 host's plugin-binary layout (distinct from
//!      the plugin's own STATE root at
//!      `<base>/plugins-state/psychological-operations/`).
//!   2. Spawns the `objectiveai` host binary with
//!      `plugins run --owner … --name psychological-operations
//!      --version … --args '["<subcmd>", …]'` — the real v2.1.1
//!      dispatch path (the legacy `objectiveai <plugin-name> …`
//!      top-level alias is gone).
//!   3. Captures stdout + stderr. At v2.1.1 the host forwards our
//!      plugin's JSONL essentially verbatim (untagged
//!      ResponseItem; no `{"type":"begin"}`/`{"type":"end"}`
//!      bookends, those died with 2.0.x), and re-emits any plugin
//!      stderr line as a bare `{"type":"error","message":null}`
//!      stdout item.
//!   4. Asserts against committed snapshots under
//!      `assets/<name>/{stdout,stderr}.txt`.
//!
//! Each test asset folder is laid out:
//!   assets/<name>/
//!   ├── .objectiveai/                                   # initial state (committed)
//!   │   └── plugins-state/psychological-operations/...        # our state lives here
//!   ├── stdout.txt                                      # expected stdout
//!   └── stderr.txt                                      # expected stderr
//!
//! ## Embedded postgres
//!
//! Every v2.1.1 host invocation bootstraps an embedded postgres
//! under its CONFIG_BASE_DIR before doing anything else: binaries
//! extracted to `db-bin/` (~163 MB, from bytes bundled inside the
//! cli binary — no network), cluster data at `db/`, and a
//! postmaster that deliberately OUTLIVES the cli process.
//! Per-test that would mean gigabytes of extracts per run and a
//! leaked postmaster per test, so the harness:
//!   - warms up ONE shared install under
//!     `tests/.target-binaries/pg-warmup/db-bin` (see
//!     [`pg_shared_install`]) and links it into each test base as
//!     `db-bin` — postgresql_embedded skips the extract when
//!     `installation_dir` already exists; cluster data stays
//!     per-test;
//!   - stops the per-test postmaster in `Drop` (see
//!     [`kill_postmaster`]) before wiping the runtime dir.
//!
//! Tests run in PARALLEL — env vars are per-subprocess
//! (`Command::env`), never set on the test process itself.
//! Drop wipes the runtime copy on completion (or
//! `PSYOPS_KEEP_TEST_STATE=1` to preserve for debugging — the
//! postmaster is stopped either way).

#![allow(dead_code)]   // Helpers used by individual test files.

pub mod snapshot;

use std::io::Read as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Windows: clear `HANDLE_FLAG_INHERIT` on this test process's
/// stdio handles before spawning anything. cargo runs the test
/// binary with piped stdio, and those pipe handles are marked
/// inheritable — every grandchild spawned with
/// `bInheritHandles=TRUE` (which Rust's `Command` uses whenever
/// stdio is piped) would inherit them as stray raw handles. The
/// embedded postmaster the objectiveai host leaves running is
/// exactly such a grandchild: with the flag set it keeps cargo's
/// stdout pipe open forever and the outer `cargo test` never sees
/// EOF. Mirrors `objectiveai_cli::clear_stdio_inheritance`, which
/// solves the same leak one level down.
#[cfg(windows)]
fn clear_stdio_inheritance() {
    use std::os::windows::io::AsRawHandle;
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    unsafe extern "system" {
        fn SetHandleInformation(
            handle: *mut core::ffi::c_void,
            mask: u32,
            flags: u32,
        ) -> i32;
    }
    let handles = [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ];
    for h in handles {
        if !h.is_null() {
            unsafe { SetHandleInformation(h.cast(), HANDLE_FLAG_INHERIT, 0) };
        }
    }
}

#[cfg(not(windows))]
fn clear_stdio_inheritance() {}

/// Run [`clear_stdio_inheritance`] exactly once, before the first
/// subprocess spawn of the test process.
fn ensure_stdio_not_inherited() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(clear_stdio_inheritance);
}

fn manifest_dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }
fn repo_root() -> PathBuf { manifest_dir().join("..") }
fn tests_dir() -> PathBuf { manifest_dir().join("tests") }
fn assets_dir() -> PathBuf { manifest_dir().join("assets") }
fn target_binaries_dir() -> PathBuf { tests_dir().join(".target-binaries") }

/// Run `psychological-operations-browser/scripts/build-bundle.{ps1,sh}`
/// once per cargo-test process so the CLI's build.rs finds the
/// embedded browser bundle. Idempotent — the script overwrites
/// the same paths each time. Debug profile: tests don't need an
/// optimized browser, and the release CEF build is enormous.
/// The browser crate's CEF build needs `ninja` — prepend the
/// repo-root `bin/` (where `install-bin.sh` drops it) to the
/// script's PATH.
fn ensure_browser_bundle() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        ensure_stdio_not_inherited();
        let target = host_triple();
        let bin_dir = repo_root().join("bin");
        let path_var = match std::env::var_os("PATH") {
            Some(p) => {
                let mut parts = vec![bin_dir.clone()];
                parts.extend(std::env::split_paths(&p));
                std::env::join_paths(parts).expect("join PATH")
            }
            None => bin_dir.clone().into(),
        };
        let status = if cfg!(windows) {
            // Prefer pwsh (PowerShell 7); fall back to Windows PowerShell.
            let shell = if Command::new("pwsh").arg("-NoProfile").arg("-Command").arg("$null").status().map(|s| s.success()).unwrap_or(false) {
                "pwsh"
            } else {
                "powershell"
            };
            Command::new(shell)
                .args(["-NoProfile", "-File", "psychological-operations-browser/scripts/build-bundle.ps1"])
                .arg("-Target").arg(target)
                .env("PATH", &path_var)
                .current_dir(repo_root())
                .status()
        } else {
            Command::new(bash_command())
                .arg("psychological-operations-browser/scripts/build-bundle.sh")
                .arg("--target").arg(target)
                .env("PATH", &path_var)
                .current_dir(repo_root())
                .status()
        }
        .expect("spawn build-bundle script");
        assert!(status.success(), "psychological-operations-browser build-bundle failed");
    });
}


/// Build our `psychological-operations` binary once per cargo-test
/// process. Subsequent calls return the cached path. Debug profile
/// — matches the bundle above, and the plugin's runtime speed is
/// irrelevant to the assertions.
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
                "--target-dir", target.to_str().unwrap(),
                "--manifest-path", manifest_dir().join("Cargo.toml").to_str().unwrap(),
            ])
            .status()
            .expect("spawn cargo build psychological-operations");
        assert!(status.success(), "psychological-operations build failed");
        let exe = if cfg!(windows) { "psychological-operations.exe" } else { "psychological-operations" };
        target.join("debug").join(exe)
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
const OBJECTIVEAI_VERSION: &str = "2.1.1";

/// Plugin coordinate for the manual test install. The v2.1.1 host
/// resolves plugin binaries at
/// `<base>/plugins/<owner>/<name>/<version>/plugin[.exe]`; for a
/// hand-placed binary the owner/version are arbitrary as long as
/// the `plugins run` invocation passes the same triple. No
/// `objectiveai.json` manifest is needed — `plugins run` only
/// resolves the binary path.
const PLUGIN_OWNER:   &str = "ObjectiveAI";
const PLUGIN_NAME:    &str = "psychological-operations";
const PLUGIN_VERSION: &str = "0.0.0";

/// Filename for the prebuilt `objectiveai` release asset on the
/// current host, matching the upload convention in
/// `objectiveai/.github/workflows/release.yml`. v2.1.1 ships the
/// viewer as its own `-viewer` asset, so the plain cli asset is
/// already viewer-free (the 2.0.x `-no-viewer` variant is gone).
fn objectiveai_asset_name() -> &'static str {
    if      cfg!(all(target_os = "windows", target_arch = "x86_64"))  { "objectiveai-windows-x86_64.exe" }
    else if cfg!(all(target_os = "macos",   target_arch = "aarch64")) { "objectiveai-macos-aarch64" }
    else if cfg!(all(target_os = "macos",   target_arch = "x86_64"))  { "objectiveai-macos-x86_64" }
    else if cfg!(all(target_os = "linux",   target_arch = "aarch64")) { "objectiveai-linux-aarch64" }
    else if cfg!(all(target_os = "linux",   target_arch = "x86_64"))  { "objectiveai-linux-x86_64" }
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
            // No client timeout: the v2.1.1 asset embeds the
            // postgres archive (hundreds of MB) and reqwest's
            // default 30 s window kills the body mid-stream
            // ("error decoding response body").
            let client = reqwest::blocking::Client::builder()
                .timeout(None)
                .build()
                .expect("build download client");
            let bytes = client
                .get(&url)
                .send()
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

/// Release-asset filename for the `objectiveai-api` mock server on
/// the current host. At v2.1.1 the API server is its own asset
/// (`-api`), split out from the plain cli binary. It serves the
/// `remote:mock` function / profile definitions and deterministic
/// mock executions the psyop scoring path drives through the host's
/// `functions get` / `executions create` — so without it, those
/// nested commands hit the real `https://api.objectiveai.dev` and
/// fail.
fn objectiveai_api_asset_name() -> &'static str {
    if      cfg!(all(target_os = "windows", target_arch = "x86_64"))  { "objectiveai-windows-x86_64-api.exe" }
    else if cfg!(all(target_os = "macos",   target_arch = "aarch64")) { "objectiveai-macos-aarch64-api" }
    else if cfg!(all(target_os = "macos",   target_arch = "x86_64"))  { "objectiveai-macos-x86_64-api" }
    else if cfg!(all(target_os = "linux",   target_arch = "aarch64")) { "objectiveai-linux-aarch64-api" }
    else if cfg!(all(target_os = "linux",   target_arch = "x86_64"))  { "objectiveai-linux-x86_64-api" }
    else { panic!("unsupported host platform — extend objectiveai_api_asset_name()") }
}

/// Download (once) and cache the `objectiveai-api` server binary for
/// `v<OBJECTIVEAI_VERSION>`, mirroring [`objectiveai_binary`].
pub fn objectiveai_api_binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let cache_dir = target_binaries_dir().join("objectiveai-release");
        std::fs::create_dir_all(&cache_dir).expect("create objectiveai cache dir");
        let asset = objectiveai_api_asset_name();
        let cached = cache_dir.join(format!("objectiveai-v{OBJECTIVEAI_VERSION}-{asset}"));
        if !cached.exists() {
            let url = format!(
                "https://github.com/ObjectiveAI/objectiveai/releases/download/v{OBJECTIVEAI_VERSION}/{asset}",
            );
            eprintln!("downloading objectiveai-api v{OBJECTIVEAI_VERSION}: {url}");
            let client = reqwest::blocking::Client::builder()
                .timeout(None)
                .build()
                .expect("build download client");
            let bytes = client
                .get(&url)
                .send()
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

/// Fixed loopback port for the SHARED mock API server. Cargo runs
/// each integration-test FILE as its own process, so a per-process
/// server would mean ~16 servers; instead every process targets one
/// well-known port and the first to find it down spawns the server
/// (the rest reuse it). High/obscure to avoid collisions.
const API_SERVER_PORT: u16 = 17717;

/// Base URL of the shared mock API server, spawning it on first need.
///
/// Cross-process-safe: a lock file under `tests/.target-binaries`
/// serializes the spawn decision so two parallel test processes
/// can't both bind [`API_SERVER_PORT`]. The server is left running
/// for reuse across the whole `cargo test` invocation (like the
/// embedded postmasters the host leaves behind) — it's an in-memory
/// mock with no on-disk state. To stop it: kill the
/// `objectiveai-*-api` process.
fn api_server_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let url = format!("http://127.0.0.1:{API_SERVER_PORT}");
        // Already up (this process spawned it earlier, or a parallel
        // test process / prior run did)? Reuse.
        if api_server_responsive() {
            return url;
        }
        // Cross-process mutex via a short-lived lock FILE (held only
        // for the spawn decision, then deleted — so it can't persist
        // as a stale deadlock marker). Standard double-checked lock.
        let lock = target_binaries_dir().join("api-server.lock");
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(_) => {
                    // We hold the lock. Re-check: a parallel process
                    // may have spawned while we contended for it.
                    if !api_server_responsive() {
                        spawn_api_server();
                        for _ in 0..600 {
                            if api_server_responsive() { break; }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                    let _ = std::fs::remove_file(&lock);
                    break;
                }
                Err(_) => {
                    // Someone else holds it. If the server comes up,
                    // reuse. Break a stale lock (holder crashed) once
                    // it's older than 90 s.
                    if api_server_responsive() { break; }
                    if let Ok(age) = std::fs::metadata(&lock)
                        .and_then(|m| m.modified())
                        .and_then(|t| t.elapsed().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
                    {
                        if age > std::time::Duration::from_secs(90) {
                            let _ = std::fs::remove_file(&lock);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        if !api_server_responsive() {
            panic!("mock API server never came up on {url}");
        }
        url
    }).as_str()
}

/// TCP-probe the shared API port. A successful connect means an
/// `objectiveai-api` (or a prior run's) is listening.
fn api_server_responsive() -> bool {
    TcpStream::connect(("127.0.0.1", API_SERVER_PORT)).is_ok()
}

/// Spawn the mock API server on [`API_SERVER_PORT`], detached, and
/// block until it logs `listening on …`. Default features build an
/// in-memory persistent cache (no SQLite file, no postgres) and the
/// `remote:mock` path needs no upstream auth, so the only config is
/// the bind address/port. The child is intentionally leaked (kept
/// alive for the run); `kill_on_drop` is NOT set so it survives this
/// thread.
fn spawn_api_server() {
    use std::process::Stdio;
    let mut child = Command::new(objectiveai_api_binary())
        .env("ADDRESS", "127.0.0.1")
        .env("PORT", API_SERVER_PORT.to_string())
        // Dedicated, throwaway base dir — the default-feature build
        // keeps its cache in memory, but the binary still resolves a
        // config dir for other reads.
        .env("CONFIG_BASE_DIR", target_binaries_dir().join("api-base"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn objectiveai-api");

    // Block until the public listener logs readiness (it prints
    // "listening on <addr>" then "mcp listening on …"). Read stderr
    // on this thread up to that marker; then leak the handle so the
    // server keeps running.
    if let Some(stderr) = child.stderr.take() {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        for _ in 0..200 {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,            // server exited early
                Ok(_) if line.contains("listening on") => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        // Drain remaining stderr in the background so the pipe never
        // fills and blocks the server.
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = reader.read_to_end(&mut sink);
        });
    }
    // Leak the Child: dropping it would not kill the process (no
    // kill_on_drop), but we keep it explicitly to make the intent
    // clear and avoid any wait-on-drop.
    std::mem::forget(child);
}

/// Shared embedded-postgres install (`db-bin/`) for every test base
/// dir. Warmed up once per machine under
/// `tests/.target-binaries/pg-warmup/` by running `objectiveai
/// --help` — the host builds its Context (which bootstraps
/// postgres, extracting the bundled binaries) before arg parsing,
/// so even `--help` populates the install. The warmup postmaster
/// is stopped afterwards; only the extracted `db-bin/` is kept.
fn pg_shared_install() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let warmup = target_binaries_dir().join("pg-warmup");
        let install = warmup.join("db-bin");
        if !install.exists() {
            std::fs::create_dir_all(&warmup).expect("create pg warmup dir");
            let out = Command::new(objectiveai_binary())
                .arg("--help")
                .env("CONFIG_BASE_DIR", &warmup)
                .output()
                .expect("spawn objectiveai --help for pg warmup");
            assert!(
                out.status.success(),
                "pg warmup failed: stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
            assert!(install.exists(), "pg warmup did not produce db-bin/");
        }
        // Stop the warmup postmaster (fresh from the run above, or
        // leaked by a crashed prior test process). Its cluster under
        // `pg-warmup/db/` is never used by tests.
        kill_postmaster(&warmup);
        install
    }).as_path()
}

/// Stop the postmaster whose pid is recorded on line 1 of
/// `<base>/db/postmaster.pid`. The objectiveai host deliberately
/// leaves it running (right for a developer base dir, wrong for a
/// throwaway test dir). Identity-checked kill: the pid is only
/// killed when the OS reports its image name as postgres, so a
/// stale pid file whose pid got recycled can't take out an
/// innocent process. Best-effort throughout — a postmaster that
/// already exited is success.
fn kill_postmaster(base: &Path) {
    let pid_file = base.join("db").join("postmaster.pid");
    let Ok(content) = std::fs::read_to_string(&pid_file) else { return };
    let Some(pid) = content.lines().next().map(str::trim) else { return };
    if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    if cfg!(windows) {
        // /FI filters AND together: only a process that is BOTH this
        // pid AND named postgres.exe is killed. /T takes the worker
        // children (checkpointer, bgwriter, backends) down with the
        // postmaster — a hard-killed postmaster doesn't reliably
        // reap them on Windows, and a lingering worker keeps the
        // data dir (and any inherited pipe handles) locked.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/FI", &format!("PID eq {pid}"), "/FI", "IMAGENAME eq postgres.exe"])
            .output();
    } else {
        let name_is_postgres = Command::new("ps")
            .args(["-p", pid, "-o", "comm="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("postgres"))
            .unwrap_or(false);
        if name_is_postgres {
            let _ = Command::new("kill").args(["-9", pid]).output();
        }
    }
    // The hard kill leaves the pid file behind; drop it so a later
    // `kill_postmaster` on the same base doesn't chase a dead pid.
    let _ = std::fs::remove_file(&pid_file);
}

/// Link `link` → existing directory `target`: NTFS junction on
/// Windows (works without admin or developer mode), symlink
/// elsewhere. Used to share the postgres install across test bases.
/// Idempotent: a stale link left behind by a previous run whose
/// pre-wipe couldn't fully complete (e.g. a postmaster held the
/// base open) is removed first. Removing the link never touches
/// `target` — `remove_dir`/`remove_file` on a junction or symlink
/// drops the link itself.
fn link_dir(target: &Path, link: &Path) {
    if link.exists() || link.symlink_metadata().is_ok() {
        let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
    }
    #[cfg(windows)]
    {
        let out = Command::new("cmd")
            .arg("/C").arg("mklink").arg("/J")
            .arg(link).arg(target)
            .output()
            .expect("spawn mklink /J");
        assert!(
            out.status.success(),
            "mklink /J {} -> {} failed: {}",
            link.display(), target.display(),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("symlink shared dir");
}

/// Generic per-psyop git-init: walks `psyops_dir`, and for each
/// subdirectory containing a `psyop.json` (and no existing `.git`),
/// runs the same publish flow `psyops publish` uses. Author /
/// email / commit time are pinned so the resulting commit_sha is
/// byte-stable across machines (which is what the seeded
/// `data.db` rows reference).
///
/// Asset folders just drop in whatever psyops they need under
/// `.objectiveai/plugins-state/psychological-operations/psyops/<name>/psyop.json`;
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
        ensure_stdio_not_inherited();
        // Per-test runtime layout mirrors the live install:
        //
        //   <root>/.t-<name>/.objectiveai/                    ← CONFIG_BASE_DIR
        //   <root>/.t-<name>/.objectiveai/plugins-state/psychological-operations/
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
        let state = base.join("plugins-state").join("psychological-operations");
        std::fs::create_dir_all(&state).expect("create test state dir");

        // Copy the asset's .objectiveai/ verbatim into the runtime
        // CONFIG_BASE_DIR. Asset structure:
        //   assets/<name>/.objectiveai/plugins-state/psychological-operations/data.db
        //   assets/<name>/.objectiveai/plugins-state/psychological-operations/psyops/...
        let assets = assets_dir().join(name);
        let initial = assets.join(".objectiveai");
        if initial.exists() {
            copy_dir_recursive(&initial, &base);
        }

        // Manual plugin install: copy our built binary to the v2.1.1
        // host's plugin-binary layout
        // `plugins/<owner>/<name>/<version>/plugin[.exe]`, matching
        // what `objectiveai plugins install` produces from GitHub. We
        // use manual copy because the install command requires
        // network + a published release. (This `plugins/` tree holds
        // host-resolved BINARIES; the plugin's own state root above
        // is the separate `plugins-state/` tree.)
        let plugin_bin = if cfg!(windows) { "plugin.exe" } else { "plugin" };
        let plugin_dir = base
            .join("plugins")
            .join(PLUGIN_OWNER)
            .join(PLUGIN_NAME)
            .join(PLUGIN_VERSION);
        std::fs::create_dir_all(&plugin_dir).expect("create plugin install dir");
        std::fs::copy(psyops_binary(), plugin_dir.join(plugin_bin))
            .expect("install plugin binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = plugin_dir.join(plugin_bin);
            let mut perms = std::fs::metadata(&p).expect("plugin perms").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&p, perms).expect("chmod plugin");
        }

        // Pre-link the shared postgres install as `db-bin` so the
        // host's embedded-postgres bootstrap skips its ~163 MB
        // per-base extract (it early-returns when the installation
        // dir exists). Cluster data (`db/`) stays per-test.
        link_dir(pg_shared_install(), &base.join("db-bin"));

        let psyops_dir = state.join("psyops");
        if psyops_dir.exists() {
            git_init_psyops(&psyops_dir);
        }

        Self { name: name.into(), dir: state, base, assets }
    }

    /// Build a `Command` for invoking the objectiveai host with this
    /// test's env. Per-subprocess env, not per-process.
    pub fn cmd(&self) -> Command {
        let mut cmd = Command::new(objectiveai_binary());
        cmd.env("CONFIG_BASE_DIR",                              &self.base);
        // Point the host (and the nested `functions get` /
        // `executions create` it runs on the plugin's behalf) at the
        // local mock API server instead of the production endpoint.
        // Spawns the shared server on first use.
        cmd.env("OBJECTIVEAI_ADDRESS",                          api_server_url());
        // The host stamps OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY on the
        // plugin subprocess from its own config (default "cli") —
        // pin it so anything that records the caller's hierarchy
        // (e.g. `agents enqueue`'s deliverer column) is
        // deterministic in snapshots.
        cmd.env("OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY",         "test-harness");
        // X mocking moved from a process-wide env var to a
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

    /// Run one plugin invocation through the host's real dispatch
    /// path — `objectiveai plugins run --owner … --name … --version
    /// … --args '["<subcmd>", …]'` — and capture stdout + stderr.
    /// (The 2.0.x `objectiveai psychological-operations <subcmd>`
    /// top-level alias no longer exists.)
    pub fn run(&self, args: &[&str]) -> CapturedOutput {
        let args_json = serde_json::to_string(args)
            .expect("plugin args serialize");
        let out = self.cmd()
            .args([
                "plugins", "run",
                "--owner",   PLUGIN_OWNER,
                "--name",    PLUGIN_NAME,
                "--version", PLUGIN_VERSION,
                "--args",    &args_json,
            ])
            .output()
            .expect("spawn objectiveai plugins run");
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
        // The host left a per-base postmaster running by design —
        // stop ours before (maybe) deleting its data dir out from
        // under it. Applies to the keep-state path too: preserved
        // state shouldn't leak a live process.
        kill_postmaster(&self.base);

        // Wipe the entire runtime dir (parent of `.objectiveai`),
        // not just `self.base` — keeps the temp root clean between
        // runs.
        let runtime = self.base.parent().unwrap_or(&self.base).to_path_buf();
        if std::env::var_os("PSYOPS_KEEP_TEST_STATE").is_some() {
            eprintln!(
                "PSYOPS_KEEP_TEST_STATE — leaving {}",
                runtime.display(),
            );
            return;
        }
        // The killed postmaster's handles release asynchronously on
        // Windows — retry the wipe a few times before giving up.
        for attempt in 0..5 {
            match std::fs::remove_dir_all(&runtime) {
                Ok(()) => return,
                Err(_) if attempt < 4 => {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(e) => eprintln!(
                    "warning: failed to wipe {}: {e}",
                    runtime.display(),
                ),
            }
        }
    }
}
