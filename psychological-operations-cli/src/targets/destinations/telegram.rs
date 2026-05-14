use super::{json_body, Subject};

const MAX_MESSAGE_LENGTH: usize = 4096;

/// Extract the `<handle>` from `https://x.com/<handle>/status/...`.
fn parse_handle(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://x.com/")?;
    let (handle, _) = rest.split_once('/')?;
    Some(handle.to_string())
}

pub async fn send(
    bot_token: &str,
    chat_id: &str,
    subject: &Subject<'_>,
) -> Result<(), crate::error::Error> {
    let (header_plain, items) = json_body::lines(subject);
    let header = format!("*{header_plain}*");
    // Telegram-flavoured per-line: backtick the score and link the
    // URL with the @handle as anchor text (parsed back out of the
    // `https://x.com/<handle>/status/<id>` URL).
    let lines: Vec<String> = items.into_iter().map(|(label, url)| {
        let handle = parse_handle(&url).unwrap_or_else(|| url.clone());
        format!("`{label}` - [@{handle}]({url})")
    }).collect();

    let url = format!("https://api.telegram.org/bot{bot_token}/sendMessage");
    let client = reqwest::Client::new();
    for chunk in split_lines(&header, &lines, MAX_MESSAGE_LENGTH) {
        let res = client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": chunk,
                "parse_mode": "Markdown",
            }))
            .send()
            .await?;
        if !res.status().is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(crate::error::Error::Other(format!("telegram api failed: {body}")));
        }
    }
    Ok(())
}

/// Pack `header` plus per-post `lines` into messages of at most `max_len` chars,
/// splitting on line boundaries. Header is sent once, in the first message only.
/// Lines longer than `max_len` on their own are hard-truncated with "...".
fn split_lines(header: &str, lines: &[String], max_len: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut current = String::from(header);
    for line in lines {
        let candidate_len = current.len() + 1 + line.len();
        if candidate_len <= max_len {
            current.push('\n');
            current.push_str(line);
            continue;
        }
        if !current.is_empty() {
            messages.push(std::mem::take(&mut current));
        }
        if line.len() <= max_len {
            current.push_str(line);
        } else {
            let mut truncated = line.clone();
            truncated.truncate(max_len - 3);
            truncated.push_str("...");
            messages.push(truncated);
        }
    }
    if !current.is_empty() {
        messages.push(current);
    }
    messages
}
