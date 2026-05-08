//! Chrome-extension ingestion pathway. The `native-host` subcommand
//! implements Chrome's native-messaging protocol over stdin/stdout
//! and writes captured tweets into the local DB with `for_you = true`
//! and `(psyop, commit)` resolved from environment variables.

pub mod extract;
pub mod identity;
pub mod native_host;

pub async fn run(cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
    native_host::run(cfg).await
}
