//! Wire schema for the post_ids the Chromium extension sends across
//! the native-messaging port. The extension's only job is to
//! announce "I saw this id in for-you"; the Rust runtime later
//! hydrates the full post (engagement counts, text, media) via the
//! X v2 API after pulling the id off `for_you_queue`.

use serde::Deserialize;

/// One tweet id as serialized by
/// `psychological-operations-chromium-extension/content_script.js`.
/// Serde ignores unknown fields by default, so older extension
/// builds that still emit `{id, handle, text, …}` decode cleanly
/// into this — we just keep the id and drop the rest.
#[derive(Debug, Deserialize)]
pub struct IncomingPostId {
    pub id: String,
}

impl IncomingPostId {
    /// `Ok(id)` if the id is non-empty after trim, `Err(reason)`
    /// otherwise. Native-host counts errors as "skipped".
    pub fn into_id(self) -> Result<String, &'static str> {
        if self.id.trim().is_empty() {
            return Err("missing id");
        }
        Ok(self.id)
    }
}
