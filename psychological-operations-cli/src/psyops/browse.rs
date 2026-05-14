//! `psyops browse [--name <X>] [--commit <sha>]` — open chromium
//! for each psyop in turn so the operator can scroll x.com and
//! capture tweets via the extension. Blocks on each chromium's
//! exit before moving to the next psyop. With `--name <X>`,
//! opens just that one and blocks on its exit.

use crate::chromium::{extract::ensure_extracted, launch, native_host, paths::profile_dir};
use crate::error::Error;

pub async fn run(
    name_filter: Option<&str>,
    commit_filter: Option<&str>,
    cfg: &crate::run::Config,
) -> Result<crate::Output, Error> {
    let materialized = ensure_extracted(cfg)?;
    crate::emit::emit(crate::events::Event::BrowseChromiumMaterialized {
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
        return Ok(crate::Output::Empty);
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

        let profile = profile_dir(name, cfg);
        std::fs::create_dir_all(&profile)?;
        native_host::install(&profile, cfg)?;

        let mut child = launch::spawn(
            &materialized.chromium_binary,
            &materialized.scrape_extension_dir,
            &profile,
            name,
            &commit,
            "https://x.com/home",
        )?;

        // Block until the operator closes chromium. `wait` is sync;
        // that's fine here — the runtime is operator-paced.
        let status = child.wait().map_err(|e| {
            Error::Other(format!("waiting for chromium ({name}) failed: {e}"))
        })?;
        crate::emit::emit(crate::events::Event::BrowseChromiumExit {
            psyop: name.to_string(),
            status: status.code(),
        });
    }

    Ok(crate::Output::Empty)
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
