use serde::Serialize;

use super::Subject;

/// Top-level JSON body shared by destinations that emit a structured payload
/// (currently `http`, the `json` mode of `stdout`/`stderr`/`file`, and `exec`).
/// Tagged on `kind` so consumers can branch on the subject type.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body<'a> {
    Psyop {
        name: &'a str,
        results: Vec<Result_<'a>>,
    },
}

/// One scored tweet in the psyop body. Only the id / handle / score
/// survive into the delivery queue, so that is all a delivered result
/// carries (plus the derived URL).
#[derive(Debug, Serialize)]
#[serde(rename = "Result")]
pub struct Result_<'a> {
    pub score: f64,
    pub id: &'a str,
    pub handle: &'a str,
    pub url: String,
}

pub fn build<'a>(subject: &'a Subject<'a>) -> Body<'a> {
    match subject {
        Subject::Psyop { name, psyop: _, output } => Body::Psyop {
            name,
            results: output.iter().map(|s| Result_ {
                score: s.score,
                id: &s.id,
                handle: &s.handle,
                url: format!("https://x.com/{}/status/{}", s.handle, s.id),
            }).collect(),
        },
    }
}

/// Helper for text-mode destinations: flatten the subject into a header line
/// plus a list of (label, url) pairs.
pub fn lines(subject: &Subject) -> (String, Vec<(String, String)>) {
    match subject {
        Subject::Psyop { name, output, .. } => {
            let header = format!("PsyOp \"{name}\"");
            let lines = output.iter().map(|s| {
                let url = format!("https://x.com/{}/status/{}", s.handle, s.id);
                (format!("{:.4}", s.score), url)
            }).collect();
            (header, lines)
        }
    }
}
