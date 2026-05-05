//! Port-file paths used by the `agent reply` / `agent list` CLI commands.
//!
//! The wait-for-reply receiver was removed alongside the playwright-based
//! scraper. The CLI commands that *send* replies and enumerate active
//! interventions still need the canonical paths, so those helpers stay.

use std::path::PathBuf;

fn agent_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".psychological-operations")
}

pub fn pid_port_file(pid: u32) -> PathBuf {
    agent_dir().join(format!("agent-{pid}.port"))
}

pub fn scrape_port_file(name: &str) -> PathBuf {
    agent_dir().join(format!("agent-scrape-{name}.port"))
}

/// Glob-style helper used by `agent list` to enumerate active interventions.
pub fn agent_dir_path() -> PathBuf {
    agent_dir()
}
