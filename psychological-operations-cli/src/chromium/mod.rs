//! Embedded-Chromium subsystem.
//!
//! The Chromium zip and the packed extension live inside
//! the Rust binary via `include_bytes!`. On `psychological-operations
//! psyops browse [--name <X>]`:
//!
//!   1. (psyop, commit) are resolved (commit defaults to git HEAD).
//!   2. The Chromium zip + extension are content-hash-extracted into
//!      ~/.psychological-operations/chromium/<hash>/ on first run.
//!   3. The native-messaging host is registered (HKCU registry on
//!      Windows, NativeMessagingHosts dir under the profile on
//!      Linux/macOS), pointing at a wrapper script that invokes us
//!      with the `native-host` subcommand.
//!   4. Chromium is spawned with --user-data-dir=<per-psyop profile>,
//!      --load-extension=<extracted ext>, and PSYOP_NAME /
//!      PSYOP_COMMIT_SHA on the env so the eventual native-host
//!      child inherits identity.
//!
//! `oauth::setup` reuses the same bundle / profile / native-host
//! plumbing but lands on the X authorize URL and discards the
//! Chromium Child to keep the OAuth dance asynchronous.

pub mod bundles;
pub mod extract;
pub mod launch;
pub mod native_host;
pub mod paths;
pub mod pinned;
