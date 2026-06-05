use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
pub struct Exec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub mode: Mode,
}

pub async fn send(cfg: &Exec, subject: &Subject<'_>) -> Result<(), crate::error::Error> {
    let payload = render(&cfg.mode, subject)?;

    let mut cmd = Command::new(&cfg.program);
    cmd.args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }

    let mut child = cmd.spawn()
        .map_err(|e| crate::error::Error::Other(format!("exec spawn failed: {e}")))?;

    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            crate::error::Error::Other("exec child has no stdin".into())
        })?;
        stdin.write_all(payload.as_bytes())?;
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(crate::error::Error::Other(format!(
            "exec child \"{}\" exited with {status}", cfg.program,
        )));
    }
    Ok(())
}

fn render(mode: &Mode, subject: &Subject) -> Result<String, crate::error::Error> {
    let mut s = String::new();
    match mode {
        Mode::Urls => {
            let (_, lines) = json_body::lines(subject);
            for (_, url) in lines {
                s.push_str(&url);
                s.push('\n');
            }
        }
        Mode::UrlsWithScores => {
            let (_, lines) = json_body::lines(subject);
            for (label, url) in lines {
                s.push_str(&format!("{label} — {url}\n"));
            }
        }
        Mode::Json => {
            let body = json_body::build(subject);
            s.push_str(&serde_json::to_string(&body)?);
            s.push('\n');
        }
    }
    Ok(s)
}
