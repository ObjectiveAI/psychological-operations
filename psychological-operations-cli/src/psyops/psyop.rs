//! Disk I/O for psyop definitions. The `PsyOp` struct + its
//! `validate` / `mock_enabled` methods live in
//! `psychological_operations_sdk::cli::psyops::psyop`; this file
//! is purely the load/save layer that touches git/fs.

use super::PsyOp;

/// Read a psyop's JSON definition.
///
/// `commit_sha = None` → working-tree read of
/// `<psyops_dir>/<name>/psyop.json`.
/// `commit_sha = Some(sha)` → walk the named commit's tree in
/// `<psyops_dir>/<name>/`'s git repo and parse `psyop.json`'s blob.
/// Doesn't touch the working tree.
pub fn load(name: &str, commit_sha: Option<&str>, ctx: &crate::context::Context) -> Result<PsyOp, crate::error::Error> {
    use crate::error::Error;
    let dir = crate::config::psyops_dir(&ctx.config).join(name);

    let bytes: Vec<u8> = match commit_sha {
        None => {
            let path = dir.join("psyop.json");
            if !path.exists() {
                return Err(Error::PsyopNotFound(path.display().to_string()));
            }
            std::fs::read(&path)?
        }
        Some(sha) => {
            let repo = git2::Repository::open(&dir).map_err(|e| {
                Error::Other(format!("git open failed at {}: {e}", dir.display()))
            })?;
            let oid = git2::Oid::from_str(sha).map_err(|e| {
                Error::Other(format!("invalid commit sha \"{sha}\": {e}"))
            })?;
            let commit = repo.find_commit(oid).map_err(|e| {
                Error::Other(format!("commit {sha} not found in {}: {e}", dir.display()))
            })?;
            let tree = commit.tree().map_err(|e| {
                Error::Other(format!("commit {sha} tree lookup failed: {e}"))
            })?;
            let entry = tree.get_path(std::path::Path::new("psyop.json"))
                .map_err(|_| Error::PsyopNotFound(format!(
                    "{}@{sha}:psyop.json", dir.display(),
                )))?;
            let object = entry.to_object(&repo).map_err(|e| {
                Error::Other(format!("psyop.json blob lookup failed at {sha}: {e}"))
            })?;
            let blob = object.as_blob().ok_or_else(|| {
                Error::Other(format!("psyop.json at {sha} is not a blob"))
            })?;
            blob.content().to_vec()
        }
    };

    Ok(serde_json::from_slice(&bytes)?)
}

/// Write a psyop's JSON definition back to disk (pretty-printed).
pub fn save(name: &str, psyop: &PsyOp, ctx: &crate::context::Context) -> Result<(), crate::error::Error> {
    let path = crate::config::psyops_dir(&ctx.config).join(name).join("psyop.json");
    let json = serde_json::to_string_pretty(psyop)?;
    std::fs::write(&path, json + "\n")?;
    Ok(())
}
