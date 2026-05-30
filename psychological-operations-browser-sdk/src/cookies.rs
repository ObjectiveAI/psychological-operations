//! On-disk readers for CEF cookie state.
//!
//! The runtime browser process owns its own live cookie view via
//! CEF's `CookieManager`. This module is for *external* callers
//! (CLI, future `get_auth_data` wrapper) that need to ask "who is
//! signed in?" without being in-process — they read the CEF
//! cookies SQLite store directly off disk and decrypt the values
//! the same way Chromium does.
//!
//! Only the `twid`→user-id reader is exported (privately, for
//! now) — a forthcoming `get_auth_data(...)` wrapper will be the
//! public entry point and will bundle in the auth token + stored
//! OAuth creds.
//!
//! The only public item today is [`parse_twid`], which the
//! browser's runtime `cookies_watcher` also re-uses so both
//! views of the cookie stay in sync at the parsing layer.

use std::path::Path;

use crate::mode::Mode;

/// `twid` is shaped `u%3D<numeric-id>` (URL-encoded `u=<id>`).
/// Pull out the digits. Match both the URL-encoded and decoded
/// prefixes — different consumers of the cookie store may or may
/// not URL-decode the raw value for us.
pub fn parse_twid(raw: &str) -> Option<String> {
    let id = raw
        .strip_prefix("u%3D")
        .or_else(|| raw.strip_prefix("u="))?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(id.to_string())
}

/// Errors `signed_in_x_user_id` can return. Wraps the underlying
/// failure modes (I/O, SQLite, JSON parse of `Local State`, key
/// material missing, DPAPI failure, AES-GCM failure).
#[derive(Debug)]
pub(crate) enum CookiesError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    LocalState(serde_json::Error),
    Base64(base64::DecodeError),
    /// `os_crypt.encrypted_key` missing or not a string in `Local State`.
    KeyMissing,
    /// `encrypted_key` decoded bytes didn't start with the literal "DPAPI" prefix.
    KeyFormat,
    /// Cookie `encrypted_value` had a version prefix we don't decrypt
    /// (today: anything other than `v10` — `v11` is Chromium's
    /// app-bound scheme and not implemented here yet). String holds
    /// the prefix for diagnostics.
    UnsupportedPrefix(String),
    /// AES-GCM authentication tag check failed — wrong key, corrupt
    /// blob, or the ciphertext is shorter than nonce+tag.
    AesGcm,
    #[cfg(windows)]
    Dpapi(windows::core::Error),
    #[cfg(not(windows))]
    UnsupportedPlatform,
}

impl std::fmt::Display for CookiesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::LocalState(e) => write!(f, "Local State parse: {e}"),
            Self::Base64(e) => write!(f, "base64: {e}"),
            Self::KeyMissing => write!(f, "os_crypt.encrypted_key missing from Local State"),
            Self::KeyFormat => write!(f, "encrypted_key did not start with the DPAPI prefix"),
            Self::UnsupportedPrefix(p) => {
                write!(f, "unsupported cookie encryption prefix {p:?}")
            }
            Self::AesGcm => write!(f, "AES-GCM decryption failed (tag mismatch or short blob)"),
            #[cfg(windows)]
            Self::Dpapi(e) => write!(f, "DPAPI: {e}"),
            #[cfg(not(windows))]
            Self::UnsupportedPlatform => {
                write!(f, "cookie decryption not implemented on this platform yet")
            }
        }
    }
}

impl std::error::Error for CookiesError {}

impl From<std::io::Error> for CookiesError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<rusqlite::Error> for CookiesError {
    fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) }
}
impl From<base64::DecodeError> for CookiesError {
    fn from(e: base64::DecodeError) -> Self { Self::Base64(e) }
}

/// Look up the X user-id currently signed in for `mode` by
/// reading the on-disk CEF cookies store. Returns `Ok(None)` if
/// the cookies DB doesn't exist yet (browser never opened in this
/// mode), the DB has no `twid` row, or the `twid` value doesn't
/// parse as a numeric user-id. Real I/O / decrypt failures bubble
/// up as `Err`.
///
/// Private — a forthcoming `get_auth_data(...)` will wrap this
/// alongside the auth token + stored OAuth creds and be the
/// public API.
#[allow(dead_code)] // first caller (get_auth_data) lands in a follow-up
pub(crate) fn signed_in_x_user_id(
    config_base_dir: &Path,
    mode: &Mode,
) -> Result<Option<String>, CookiesError> {
    let cef_root = config_base_dir
        .join("plugins")
        .join("psychological-operations")
        .join("browser")
        .join("cef-root");
    let cookies_db = cef_root.join(cache_subdir_for(mode)).join("Network").join("Cookies");
    let local_state = cef_root.join("Local State");

    if !cookies_db.exists() {
        return Ok(None);
    }

    let key = load_aes_key(&local_state)?;

    // Open the live DB via SQLite's URI form with
    // `mode=ro&immutable=1`. The immutable flag tells SQLite the
    // file won't change underneath it and skips all OS-level
    // locking, which is the only way to coexist with Chromium's
    // FILE_SHARE_NONE handle on Windows (a plain fs::copy or
    // OpenOptions::read fails with ERROR_SHARING_VIOLATION).
    // `cookies_uri` percent-encodes the path; SQLite's URI parser
    // requires forward slashes and `%` escaping for spaces etc.
    let uri = build_sqlite_uri(&cookies_db);
    let conn = rusqlite::Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    let mut stmt = conn.prepare(
        "SELECT encrypted_value FROM cookies \
         WHERE name = 'twid' \
           AND host_key IN ('.x.com', '.twitter.com', 'x.com', 'twitter.com') \
         ORDER BY creation_utc DESC \
         LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let blob: Vec<u8> = row.get(0)?;

    let plaintext = decrypt_value(&key, &blob)?;
    let raw = String::from_utf8_lossy(&plaintext);
    Ok(parse_twid(&raw))
}

/// CEF per-context cache subdirectory under `cef-root/`. Mirrors
/// `webview::cache_subdir_for` in the browser crate — kept as a
/// private duplicate here (≤5 lines, has to stay in sync).
fn cache_subdir_for(mode: &Mode) -> String {
    match mode {
        Mode::XApp => "x-app".to_string(),
        Mode::PsyopRead { name } | Mode::PsyopAuthorize { name } => {
            format!("psyop/{name}")
        }
    }
}

/// Parse `Local State` JSON → unwrap `os_crypt.encrypted_key` →
/// strip the `"DPAPI"` literal prefix → DPAPI-unprotect →
/// raw 32-byte AES-256-GCM key.
#[cfg(windows)]
fn load_aes_key(local_state_path: &Path) -> Result<Vec<u8>, CookiesError> {
    use base64::Engine;

    let raw = std::fs::read(local_state_path)?;
    let json: serde_json::Value = serde_json::from_slice(&raw).map_err(CookiesError::LocalState)?;
    let key_b64 = json
        .get("os_crypt")
        .and_then(|v| v.get("encrypted_key"))
        .and_then(|v| v.as_str())
        .ok_or(CookiesError::KeyMissing)?;
    let key_blob = base64::engine::general_purpose::STANDARD.decode(key_b64)?;
    if key_blob.len() < 5 || &key_blob[..5] != b"DPAPI" {
        return Err(CookiesError::KeyFormat);
    }
    dpapi_unprotect(&key_blob[5..])
}

#[cfg(not(windows))]
fn load_aes_key(_local_state_path: &Path) -> Result<Vec<u8>, CookiesError> {
    Err(CookiesError::UnsupportedPlatform)
}

/// Decrypt one cookie `encrypted_value` blob:
///   prefix(3) ‖ nonce(12) ‖ ciphertext ‖ tag(16)
/// then strip Chromium's 32-byte SHA-256 integrity prefix that
/// recent Chromium versions (≥ 127-ish) prepend to the plaintext
/// before encryption (the hash of `host_key + name`, used to
/// detect cookie-row tampering).
///
/// Currently supports `v10` only — `v11` is Chromium's newer
/// app-bound scheme and would need additional unwrap steps.
fn decrypt_value(key: &[u8], blob: &[u8]) -> Result<Vec<u8>, CookiesError> {
    use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead, Nonce};

    if blob.len() < 3 + 12 + 16 {
        return Err(CookiesError::AesGcm);
    }
    let prefix = &blob[..3];
    if prefix != b"v10" {
        return Err(CookiesError::UnsupportedPrefix(
            String::from_utf8_lossy(prefix).into_owned(),
        ));
    }
    let nonce = Nonce::from_slice(&blob[3..15]);
    let ct_and_tag = &blob[15..];
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CookiesError::AesGcm)?;
    let mut plaintext = cipher
        .decrypt(nonce, ct_and_tag)
        .map_err(|_| CookiesError::AesGcm)?;

    // Strip the 32-byte SHA-256 integrity prefix if present.
    // Older Chromium builds (< 127-ish) didn't prepend it — guard
    // by length so we don't truncate a short plaintext into
    // nothing on the legacy path.
    const INTEGRITY_PREFIX: usize = 32;
    if plaintext.len() > INTEGRITY_PREFIX {
        plaintext.drain(..INTEGRITY_PREFIX);
    }
    Ok(plaintext)
}

#[cfg(windows)]
fn dpapi_unprotect(input: &[u8]) -> Result<Vec<u8>, CookiesError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CryptUnprotectData,
    };

    let mut input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &mut input_blob,
            None,
            None,
            None,
            None,
            0,
            &mut output_blob,
        )
        .map_err(CookiesError::Dpapi)?;
    }
    let out = unsafe {
        std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec()
    };
    // CryptUnprotectData allocates with LocalAlloc; release with LocalFree.
    let _ = unsafe { LocalFree(Some(HLOCAL(output_blob.pbData.cast()))) };
    Ok(out)
}

/// Build a `file:` URI suitable for `Connection::open_with_flags`
/// + `SQLITE_OPEN_URI`. SQLite's URI parser requires:
///   - forward slashes (Windows `\` → `/`)
///   - leading `/` (so an absolute Windows path like `C:\foo`
///     becomes `file:/C:/foo`)
///   - percent-encoding of characters with special meaning in
///     URIs (we encode space, `?`, `#`, and `%` itself; the rest
///     of the printable ASCII in a typical filesystem path is
///     safe to pass through).
///
/// `mode=ro&immutable=1` tells SQLite the file is read-only AND
/// won't change underneath it — bypasses all OS-level locking and
/// works against a Cookies DB that Chromium holds with
/// FILE_SHARE_NONE.
fn build_sqlite_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut encoded = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3F"),
            '#' => encoded.push_str("%23"),
            ' ' => encoded.push_str("%20"),
            other => encoded.push(other),
        }
    }
    // Absolute Windows paths start with a drive letter; SQLite
    // wants `file:/C:/...`. POSIX paths already start with `/`.
    if encoded.starts_with('/') {
        format!("file:{encoded}?mode=ro&immutable=1")
    } else {
        format!("file:/{encoded}?mode=ro&immutable=1")
    }
}
