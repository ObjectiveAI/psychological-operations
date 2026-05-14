use serde::{Deserialize, Serialize};

use crate::events::Transport;
use super::Subject;

/// Mirror of [`super::stdout::Mode`]. Kept as its own type so config
/// serialization stays distinct (`{"type":"stderr","mode":…}` vs
/// `{"type":"stdout",…}`), but the actual delivery logic shares
/// [`super::stdout::deliver`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Urls,
    UrlsWithScores,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stderr {
    pub mode: Mode,
}

pub async fn send(cfg: &Stderr, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    // Reuse stdout::deliver — only the `transport` discriminator on the
    // emitted `Event::TargetDelivered` differs.
    let mirror = super::stdout::Stdout {
        mode: match cfg.mode {
            Mode::Urls => super::stdout::Mode::Urls,
            Mode::UrlsWithScores => super::stdout::Mode::UrlsWithScores,
            Mode::Json => super::stdout::Mode::Json,
        },
    };
    super::stdout::deliver(&mirror, subject, Transport::Stderr)
}
