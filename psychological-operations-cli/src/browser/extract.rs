//! Content-hash-keyed cached extraction of the embedded browser zip
//! into `<base_dir>/browser-cache/<hash>/`. First call materializes
//! the bundle; subsequent calls short-circuit when the hash dir
//! already contains a `.ready` sentinel.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::bundle::{BROWSER_BUNDLE, browser_entry};
use crate::error::Error;

/// Materialized layout returned to the launcher.
pub struct Extracted {
    pub root: PathBuf,
    pub binary: PathBuf,
}

/// Extract (or hit cache) and return the relevant paths.
pub fn ensure_extracted(cfg: &crate::run::Config) -> Result<Extracted, Error> {
    let hash = content_hash();
    let root = browser_cache_root(cfg).join(format!("{hash:016x}"));
    let binary = root.join(browser_entry());
    let sentinel = root.join(".ready");

    if !sentinel.exists() {
        if root.exists() {
            // Stale partial extraction — start fresh so we never
            // leave half-extracted files behind on the second
            // attempt.
            let _ = fs::remove_dir_all(&root);
        }
        fs::create_dir_all(&root)?;
        extract_zip(BROWSER_BUNDLE, &root)?;

        // Cross-platform Unix executable bit on the launcher binary.
        // The zip on Windows is built with Compress-Archive, which
        // doesn't preserve unix modes; set it explicitly.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if binary.exists() {
                let mut perms = fs::metadata(&binary)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&binary, perms)?;
            }
        }

        fs::write(&sentinel, "ok")?;
    }

    Ok(Extracted { root, binary })
}

/// Cache root for the extracted browser bundle. Each unique embedded
/// payload (content-hashed) gets its own subdirectory.
pub fn browser_cache_root(cfg: &crate::run::Config) -> PathBuf {
    cfg.state_dir().join("browser-cache")
}

fn content_hash() -> u64 {
    let mut hasher = Sha256::new();
    hasher.update((BROWSER_BUNDLE.len() as u64).to_le_bytes());
    hasher.update(BROWSER_BUNDLE);
    hasher.update(browser_entry().as_bytes());
    let digest = hasher.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(buf)
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| Error::Other(format!("browser zip open: {e}")))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| Error::Other(format!("browser zip entry: {e}")))?;
        let outpath = match file.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue,
        };
        if file.is_dir() {
            fs::create_dir_all(&outpath)?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&outpath)?;
        std::io::copy(&mut file, &mut out)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = file.unix_mode() {
                let _ = fs::set_permissions(
                    &outpath,
                    fs::Permissions::from_mode(mode),
                );
            }
        }
    }
    Ok(())
}
