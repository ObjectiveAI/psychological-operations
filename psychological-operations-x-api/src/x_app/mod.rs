//! `x_app.json` — the master X dev-account App's credentials. The
//! data shape + on-disk IO live in [`config`]; the CLI subcommand
//! that drives chromium to capture the credentials in the first
//! place lives in `psychological-operations-cli` (`x_app_setup.rs`)
//! because it depends on the CLI's chromium / event-emit modules.

pub mod config;
