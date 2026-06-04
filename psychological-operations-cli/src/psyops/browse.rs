//! `psyops browse [--name <X>] [--commit <sha>]` — open the embedded
//! browser for each psyop in turn so the operator can browse x.com
//! and save tweet IDs (Save button → stdout `tweet_id` event →
//! enqueue into the local DB). Blocks on each browser's exit before
//! moving to the next psyop.

use std::io::{BufRead, BufReader};

use psychological_operations_sdk::browser::output::Output as BrowserOutput;
use psychological_operations_sdk::cli::Output;

use crate::browser::{extract::ensure_extracted, launch};
use crate::db::Db;
use crate::error::Error;

pub async fn run(
    name_filter: Option<&str>,
    commit_filter: Option<&str>,
    cfg: &crate::run::Config,
) -> Result<Output, Error> {
    let materialized = ensure_extracted(cfg)?;
    let config_base_dir = cfg.objectiveai_base_dir();

    crate::emit::emit(crate::events::Event::BrowseBrowserMaterialized {
        path: materialized.root.display().to_string(),
    });

    let names = match name_filter {
        Some(n) => {
            let n = n.trim();
            if n.is_empty() {
                return Err(Error::Other("--name cannot be empty".into()));
            }
            vec![n.to_string()]
        }
        None => {
            if commit_filter.is_some() {
                return Err(Error::Other("--commit requires --name".into()));
            }
            list_psyops(cfg)?
        }
    };

    if names.is_empty() {
        crate::emit::emit(crate::events::Event::BrowseNoPsyops);
        return Ok(Output::Empty);
    }

    crate::emit::emit(crate::events::Event::BrowsePsyopList { count: names.len() });
    for (i, name) in names.iter().enumerate() {
        let commit = match (name_filter, commit_filter) {
            (Some(_), Some(c)) => c.to_string(),
            _ => derive_commit(name, cfg)?,
        };

        crate::emit::emit(crate::events::Event::BrowseStarting {
            psyop: name.to_string(),
            commit: commit.clone(),
            index: i + 1,
            total: names.len(),
        });

        let mut child = launch::spawn(
            &materialized.binary,
            &config_base_dir,
            launch::Mode::PsyopRead { name: name.clone() },
            /* pipe_stdin  = */ false,
            /* pipe_stdout = */ true,
        )?;

        crate::emit::emit(crate::events::Event::BrowserSpawned {
            kind: "psyop_read".into(),
            name: Some(name.clone()),
            pid: child.id(),
        });

        // Stream the browser's stdout line-by-line. Each `tweet_id`
        // event lands in the for_you queue; anything else is logged
        // and dropped. Blocks until the operator closes the browser
        // window (stdout closes, we hit EOF, the loop exits).
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other("browser stdout pipe missing".into()))?;
        let db = Db::open(cfg)?;
        let mut inserted: usize = 0;
        let mut skipped: usize = 0;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<BrowserOutput>(trimmed) {
                Ok(BrowserOutput::TweetId { id }) => {
                    match db.enqueue_for_you(&id, name, &commit) {
                        Ok(true) => inserted += 1,
                        Ok(false) => skipped += 1,
                        Err(_) => skipped += 1,
                    }
                }
                // Other events are informational here — `Log`,
                // `Url`, `SignedIn`, `Panel`, `Response`, `Help`,
                // `Error`. Drop them; the browser's own stderr
                // / its panel UI is already showing the operator
                // what they need.
                Ok(_) => {}
                Err(_) => {
                    // Browser shouldn't be emitting non-JSON on
                    // stdout, but be tolerant: skip and continue.
                }
            }
        }

        let status = child.wait().map_err(|e| {
            Error::Other(format!("waiting for browser ({name}) failed: {e}"))
        })?;
        crate::emit::emit(crate::events::Event::BrowseSessionEnded {
            psyop: name.to_string(),
            status: status.code(),
            inserted,
            skipped,
        });
    }

    Ok(Output::Empty)
}

/// Enumerate psyops on disk in alphabetical order. Same dir-walk
/// rule as `psyops::list`: must have `psyop.json` + `.git`.
fn list_psyops(cfg: &crate::run::Config) -> Result<Vec<String>, Error> {
    let dir = crate::config::psyops_dir(cfg);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for ent in std::fs::read_dir(&dir)? {
        let ent = ent?;
        let path = ent.path();
        if !path.is_dir()
            || !path.join("psyop.json").exists()
            || !path.join(".git").exists()
        {
            continue;
        }
        if let Some(name) = ent.file_name().to_str().map(|s| s.to_string()) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn derive_commit(name: &str, cfg: &crate::run::Config) -> Result<String, Error> {
    let dir = crate::config::psyops_dir(cfg).join(name);
    let repo = git2::Repository::open(&dir).map_err(|e| {
        Error::Other(format!("git open failed at {}: {e}", dir.display()))
    })?;
    let head = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
        Error::Other(format!("git HEAD lookup failed for {name}: {e}"))
    })?;
    Ok(head.id().to_string())
}
