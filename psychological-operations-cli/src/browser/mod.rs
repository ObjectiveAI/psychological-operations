//! Embedded psychological-operations-browser subsystem.
//!
//! The CEF browser binary and its full runtime live inside the CLI
//! via `include_bytes!`. On `psyops browse` / `psyops login` /
//! `agents login` / `x_app setup`:
//!
//!   1. The browser zip is content-hash-extracted into
//!      `<base>/browser-cache/<hash>/` on first run.
//!   2. The extracted exe is spawned with the right mode flag
//!      plus `--config-base-dir <objectiveai_base_dir>`.
//!   3. Caller blocks on `child.wait()` for spawn-and-wait flows;
//!      `psyops browse` additionally streams stdout for `tweet_id`
//!      events and writes them to the DB.

pub mod bundle;
pub mod extract;
pub mod launch;
