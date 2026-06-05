use std::io::Write;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{json_body, Subject};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Urls,
    UrlsWithScores,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct File {
    pub mode: Mode,
    pub path: PathBuf,
}

pub async fn send(cfg: &File, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    if let Some(parent) = cfg.path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.path)?;

    match cfg.mode {
        Mode::Urls => {
            let (_, lines) = json_body::lines(subject);
            for (_, url) in lines {
                writeln!(f, "{url}")?;
            }
        }
        Mode::UrlsWithScores => {
            let (_, lines) = json_body::lines(subject);
            for (label, url) in lines {
                writeln!(f, "{label} — {url}")?;
            }
        }
        Mode::Json => {
            let body = json_body::build(subject);
            writeln!(f, "{}", serde_json::to_string(&body)?)?;
        }
    }
    Ok(())
}
