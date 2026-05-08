//! `psyops browse [--name <X>] [--commit <sha>]` — open chromium
//! for each psyop in turn so the operator can scroll x.com and
//! capture tweets via the extension. Blocks on each chromium's
//! exit before moving to the next psyop. With `--name <X>`,
//! opens just that one and blocks on its exit.

use crate::chrome::{extract::ensure_extracted, launch, native_host, paths::profile_dir};
use crate::error::Error;

pub async fn run(
    name_filter: Option<&str>,
    commit_filter: Option<&str>,
) -> Result<crate::Output, Error> {
    let materialized = ensure_extracted()?;
    eprintln!(
        "psyops browse: chrome materialized at {}",
        materialized.root.display(),
    );

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
            list_psyops()?
        }
    };

    if names.is_empty() {
        eprintln!("psyops browse: no psyops on disk");
        return Ok(crate::Output::Empty);
    }

    eprintln!("psyops browse: {} psyop(s) to step through", names.len());
    for (i, name) in names.iter().enumerate() {
        let commit = match (name_filter, commit_filter) {
            (Some(_), Some(c)) => c.to_string(),
            _ => derive_commit(name)?,
        };

        eprintln!(
            "\n[{}/{}] psyop \"{name}\" @ {commit} — close chromium when done",
            i + 1, names.len(),
        );

        let profile = profile_dir(name);
        std::fs::create_dir_all(&profile)?;
        native_host::install(&profile)?;

        let mut child = launch::spawn(
            &materialized.chrome_binary,
            &materialized.extension_dir,
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
        eprintln!("psyops browse: chromium for \"{name}\" exited (status: {status})");
    }

    Ok(crate::Output::Empty)
}

/// Enumerate psyops on disk in alphabetical order. Same dir-walk
/// rule as `psyops::list`: must have `psyop.json` + `.git`.
fn list_psyops() -> Result<Vec<String>, Error> {
    let dir = crate::config::psyops_dir();
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

fn derive_commit(name: &str) -> Result<String, Error> {
    let dir = crate::config::psyops_dir().join(name);
    let repo = git2::Repository::open(&dir).map_err(|e| {
        Error::Other(format!("git open failed at {}: {e}", dir.display()))
    })?;
    let head = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
        Error::Other(format!("git HEAD lookup failed for {name}: {e}"))
    })?;
    Ok(head.id().to_string())
}
