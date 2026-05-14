use serde::{Deserialize, Serialize};

use crate::events::{Event, TargetBody, Transport};
use super::{json_body, Subject};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Urls,
    UrlsWithScores,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stdout {
    pub mode: Mode,
}

pub async fn send(cfg: &Stdout, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    deliver(cfg, subject, Transport::Stdout)
}

/// Shared body between `stdout` and `stderr` destinations: both emit
/// `Event::TargetDelivered` events; only the `transport` discriminator
/// distinguishes them. The operator's mode (`urls` / `urls_with_scores`
/// / `json`) maps onto the matching [`TargetBody`] variant.
pub(super) fn deliver(
    cfg: &Stdout,
    subject: &Subject<'_>,
    transport: Transport,
) -> Result<(), crate::error::Error> {
    match cfg.mode {
        Mode::Urls => {
            let (_, lines) = json_body::lines(subject);
            for (_, url) in lines {
                crate::emit::emit(Event::TargetDelivered {
                    transport,
                    body: TargetBody::Urls { url },
                });
            }
        }
        Mode::UrlsWithScores => {
            let (_, lines) = json_body::lines(subject);
            for (label, url) in lines {
                // Labels come back as `format!("{:.4}", score)` from
                // `json_body::lines`; parse back to a numeric so the
                // wire carries `score` as a number rather than a
                // pre-formatted string.
                let score: f64 = label.parse().unwrap_or(0.0);
                crate::emit::emit(Event::TargetDelivered {
                    transport,
                    body: TargetBody::UrlsWithScores { score, url },
                });
            }
        }
        Mode::Json => {
            let body = json_body::build(subject);
            crate::emit::emit(Event::TargetDelivered {
                transport,
                body: TargetBody::Json { body: serde_json::to_value(&body)? },
            });
        }
    }
    Ok(())
}
