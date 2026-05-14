use serde::{Deserialize, Serialize};

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
    match cfg.mode {
        Mode::Urls => {
            let (_, lines) = json_body::lines(subject);
            for (_, url) in lines {
                println!("{url}");
            }
        }
        Mode::UrlsWithScores => {
            let (_, lines) = json_body::lines(subject);
            for (label, url) in lines {
                println!("{label} - {url}");
            }
        }
        Mode::Json => {
            let body = json_body::build(subject);
            println!("{}", serde_json::to_string(&body)?);
        }
    }
    Ok(())
}
