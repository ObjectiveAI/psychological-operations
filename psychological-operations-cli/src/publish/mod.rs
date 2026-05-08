use git2::{Repository, Signature, Time};
use std::path::Path;

use crate::run::Config;

/// Write `content` to `dir/filename` (creating `dir` and a git repo if
/// needed) and commit it. Returns the new commit's SHA.
///
/// Author name / email and the commit time are read from `cfg`
/// (which sources them from the env vars
/// `PSYCHOLOGICAL_OPERATIONS_COMMIT_{AUTHOR_NAME,AUTHOR_EMAIL,TIME}`).
/// Pinning all three yields reproducible commit SHAs — the
/// integration test harness uses this for byte-stable seed assets.
pub fn publish_file(
    dir: &Path,
    filename: &str,
    content: &str,
    message: &str,
    cfg: &Config,
) -> Result<String, crate::error::Error> {
    let repo = if dir.join(".git").exists() {
        Repository::open(dir)?
    } else {
        std::fs::create_dir_all(dir)?;
        Repository::init(dir)?
    };

    let target = dir.join(filename);
    std::fs::write(&target, content)?;

    let mut index = repo.index()?;
    index.add_path(Path::new(filename))?;
    index.write()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    let name = cfg.commit_author_name.as_deref()
        .unwrap_or("psychological-operations");
    let email = cfg.commit_author_email.as_deref()
        .unwrap_or("psyops@localhost");
    let sig = match cfg.commit_time {
        // Fixed-timestamp signature for deterministic SHAs.
        Some(secs) => Signature::new(name, email, &Time::new(secs, 0))?,
        // Wall-clock for normal operator usage.
        None => Signature::now(name, email)?,
    };
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.as_ref().map(|p| vec![p]).unwrap_or_default();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;

    Ok(oid.to_string())
}
