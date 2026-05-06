//! Content-hash-keyed cached extraction of the embedded Chrome zip
//! and extension tar. First call materializes both into
//! `~/.psychological-operations/chrome/<hash>/`; subsequent calls
//! short-circuit when the hash dir already exists.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::bundles::{CHROME_BUNDLE, EXTENSION_TAR, launch_entry};
use super::paths::chrome_cache_root;
use crate::error::Error;

/// Materialized layout returned to the launcher.
pub struct Extracted {
    pub root: PathBuf,
    pub chrome_binary: PathBuf,
    pub extension_dir: PathBuf,
}

/// Extract (or hit cache) and return the relevant paths.
pub fn ensure_extracted() -> Result<Extracted, Error> {
    let hash = content_hash();
    let root = chrome_cache_root().join(format!("{hash:016x}"));
    let chrome_binary = root.join("chrome").join(launch_entry());
    let extension_dir = root.join("extension");
    let sentinel = root.join(".ready");

    if !sentinel.exists() {
        fs::create_dir_all(&root)?;
        let chrome_root = root.join("chrome");
        if chrome_root.exists() {
            // Stale partial extraction — start fresh so we never leave
            // half-extracted files behind on the second attempt.
            let _ = fs::remove_dir_all(&chrome_root);
        }
        fs::create_dir_all(&chrome_root)?;
        extract_zip(CHROME_BUNDLE, &chrome_root)?;

        if extension_dir.exists() {
            let _ = fs::remove_dir_all(&extension_dir);
        }
        fs::create_dir_all(&extension_dir)?;
        extract_tar(EXTENSION_TAR, &extension_dir)?;

        // Cross-platform Unix executable bit — necessary on Linux/Mac
        // because zip extraction on those platforms doesn't preserve
        // the execute mode unless we set it explicitly.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&chrome_binary)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&chrome_binary, perms)?;
        }

        fs::write(&sentinel, "ok")?;
    }

    Ok(Extracted { root, chrome_binary, extension_dir })
}

fn content_hash() -> u64 {
    // 8 bytes is enough for cache-key collision resistance (~6e9
    // expected first collision). Keeps directory names short.
    let mut hasher = Sha256::new();
    hasher.update(&(CHROME_BUNDLE.len() as u64).to_le_bytes());
    hasher.update(CHROME_BUNDLE);
    hasher.update(&(EXTENSION_TAR.len() as u64).to_le_bytes());
    hasher.update(EXTENSION_TAR);
    let digest = hasher.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(buf)
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| Error::Other(format!("chrome zip open: {e}")))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| Error::Other(format!("chrome zip entry: {e}")))?;
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

fn extract_tar(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    archive
        .unpack(dest)
        .map_err(|e| Error::Other(format!("extension tar unpack: {e}")))?;
    Ok(())
}
