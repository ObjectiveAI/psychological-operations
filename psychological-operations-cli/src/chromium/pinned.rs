//! Pre-seed the per-profile `Default/Preferences` JSON so a given
//! extension shows up pinned to the Chromium toolbar on first launch
//! (and stays pinned across re-launches via idempotent merge).
//!
//! Chromium stores the toolbar pin state under
//! `extensions.pinned_extensions` (a JSON array of extension IDs) in
//! `<user-data-dir>/Default/Preferences`. Writing to that file before
//! Chromium starts is the only portable mechanism that doesn't require
//! managed-policy file placement at OS-specific paths.
//!
//! Idempotent: writing on every launch is safe — the helper merges
//! the requested IDs into whatever's already present.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::error::Error;

pub fn seed_pinned_extensions(profile: &Path, extension_ids: &[&str]) -> Result<(), Error> {
    let prefs_path = profile.join("Default").join("Preferences");
    if let Some(parent) = prefs_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut prefs: Value = if prefs_path.exists() {
        let bytes = fs::read(&prefs_path)?;
        // A corrupt or empty Preferences is recoverable — start fresh.
        // Chromium will rebuild the rest of the file on launch.
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    // Walk to extensions.pinned_extensions, creating the path if it
    // doesn't exist. serde_json::Value::pointer_mut won't auto-create
    // intermediate objects, so we do it manually.
    let extensions = prefs
        .as_object_mut()
        .ok_or_else(|| Error::Other(
            "Preferences root is not a JSON object".into(),
        ))?
        .entry("extensions")
        .or_insert_with(|| json!({}));
    let extensions = extensions
        .as_object_mut()
        .ok_or_else(|| Error::Other(
            "Preferences \"extensions\" is not an object".into(),
        ))?;
    let pinned = extensions
        .entry("pinned_extensions")
        .or_insert_with(|| json!([]));
    let pinned = pinned
        .as_array_mut()
        .ok_or_else(|| Error::Other(
            "Preferences \"extensions.pinned_extensions\" is not an array".into(),
        ))?;

    for id in extension_ids {
        let id_value = json!(id);
        if !pinned.iter().any(|v| v == &id_value) {
            pinned.push(id_value);
        }
    }

    let serialized = serde_json::to_vec(&prefs)?;
    fs::write(&prefs_path, serialized)?;
    Ok(())
}
