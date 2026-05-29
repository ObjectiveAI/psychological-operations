//! Deterministic mock for the 5 X-API endpoints used by `psyops
//! run`. Selected via `PSYCHOLOGICAL_OPERATIONS_MOCK_X_API=true`.
//!
//! Strategy: hash the call's input (path + query + body) into a
//! u64 via SHA256, and produce a JSON response shape matching the
//! endpoint's documented Response type. Same input → same output.

use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::Error;

fn hash_u64(parts: &[&str]) -> u64 {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&d[..8]);
    u64::from_le_bytes(buf)
}

fn hex8(n: u64) -> String { format!("{n:016x}") }

pub fn send<T, B>(
    method: Method,
    path: &str,
    body: Option<&B>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
    B: Serialize + ?Sized,
{
    let body_value = body
        .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    let value = dispatch(&method, path, &Value::Null, &body_value)?;
    serde_json::from_value(value).map_err(|e| Error::Other(format!(
        "mock: deserialize {method} /{path}: {e}",
    )))
}

pub fn send_with_query<T, Q>(
    method: Method,
    path: &str,
    query: &Q,
) -> Result<T, Error>
where
    T: DeserializeOwned,
    Q: Serialize + ?Sized,
{
    let q_value = serde_json::to_value(query).unwrap_or(Value::Null);
    let value = dispatch(&method, path, &q_value, &Value::Null)?;
    serde_json::from_value(value).map_err(|e| Error::Other(format!(
        "mock: deserialize {method} /{path}: {e}",
    )))
}

pub fn send_with_query_and_body<T, Q, B>(
    method: Method,
    path: &str,
    query: &Q,
    body: &B,
) -> Result<T, Error>
where
    T: DeserializeOwned,
    Q: Serialize + ?Sized,
    B: Serialize + ?Sized,
{
    let q_value = serde_json::to_value(query).unwrap_or(Value::Null);
    let body_value = serde_json::to_value(body).unwrap_or(Value::Null);
    let value = dispatch(&method, path, &q_value, &body_value)?;
    serde_json::from_value(value).map_err(|e| Error::Other(format!(
        "mock: deserialize {method} /{path}: {e}",
    )))
}

pub fn send_no_response<B>(
    method: Method,
    path: &str,
    _body: Option<&B>,
) -> Result<(), Error>
where
    B: Serialize + ?Sized,
{
    let _ = dispatch(&method, path, &Value::Null, &Value::Null)?;
    Ok(())
}

pub fn send_with_query_no_response<Q>(
    method: Method,
    path: &str,
    query: &Q,
) -> Result<(), Error>
where
    Q: Serialize + ?Sized,
{
    let q = serde_json::to_value(query).unwrap_or(Value::Null);
    let _ = dispatch(&method, path, &q, &Value::Null)?;
    Ok(())
}

fn dispatch(
    method: &Method,
    path: &str,
    query: &Value,
    body: &Value,
) -> Result<Value, Error> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match (method.clone(), segs.as_slice()) {
        (Method::GET, ["tweets", id])                 => Ok(mock_tweet_get(id)),
        (Method::GET, ["tweets", "search", "recent"]) => Ok(mock_search_recent(query)),
        (Method::GET, ["users", "me"])                => Ok(mock_users_me()),
        (Method::POST, ["users", id, "likes"])        => Ok(mock_likes_post(id, body)),
        (Method::POST, ["users", id, "retweets"])     => Ok(mock_retweets_post(id, body)),
        _ => Err(Error::Other(format!(
            "mock-x-api: unimplemented {method} /{path}",
        ))),
    }
}

fn mock_tweet_get(id: &str) -> Value {
    let h = hash_u64(&["tweet_get", id]);
    let author_id = format!("100{}", h % 1_000_000_000);
    let username = format!("mockuser_{}", hex8(h));
    json!({
        "data": {
            "id": id,
            "text": format!("mock tweet @{}", hex8(h)),
            "author_id": author_id,
            "created_at": "2026-01-01T00:00:00.000Z",
            "public_metrics": {
                "like_count":       (h % 1_000) as u64,
                "retweet_count":    ((h / 1_000) % 100) as u64,
                "reply_count":      ((h / 100_000) % 50) as u64,
                "quote_count":      0,
                "bookmark_count":   0,
                "impression_count": (h % 10_000) as u64,
            },
        },
        "includes": {
            "users": [
                { "id": author_id, "name": format!("Mock User {}", hex8(h)), "username": username }
            ]
        }
    })
}

fn mock_search_recent(query: &Value) -> Value {
    let q_str = query.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let h = hash_u64(&["search_recent", q_str]);
    let n = ((h % 5) + 1) as usize;
    let mut tweets = Vec::with_capacity(n);
    let mut users = Vec::with_capacity(n);
    for i in 0..n {
        let leaf = format!("{q_str}#{i}");
        let lh = hash_u64(&["search_tweet", &leaf]);
        let id = format!("19{:017}", lh % 100_000_000_000_000_000);
        let author_id = format!("100{}", lh % 1_000_000_000);
        let username = format!("mockuser_{}", hex8(lh));
        tweets.push(json!({
            "id": id,
            "text": format!("mock search tweet @{} for {q_str}", hex8(lh)),
            "author_id": author_id,
            "created_at": "2026-01-01T00:00:00.000Z",
            "public_metrics": {
                "like_count":       (lh % 1_000) as u64,
                "retweet_count":    ((lh / 1_000) % 100) as u64,
                "reply_count":      ((lh / 100_000) % 50) as u64,
                "quote_count":      0,
                "bookmark_count":   0,
                "impression_count": (lh % 10_000) as u64,
            },
        }));
        users.push(json!({
            "id": author_id,
            "name": format!("Mock User {}", hex8(lh)),
            "username": username,
        }));
    }
    json!({
        "data":     tweets,
        "includes": { "users": users },
        "meta":     { "result_count": n as u64 },
    })
}

fn mock_users_me() -> Value {
    json!({
        "data": {
            "id":       "1000000000000000001",
            "name":     "Mock Me",
            "username": "mock_me",
        }
    })
}

fn mock_likes_post(_user_id: &str, body: &Value) -> Value {
    let tweet_id = body.get("tweet_id").and_then(|v| v.as_str()).unwrap_or("");
    let _ = hash_u64(&["likes_post", tweet_id]);
    json!({ "data": { "liked": true } })
}

fn mock_retweets_post(_user_id: &str, body: &Value) -> Value {
    let tweet_id = body.get("tweet_id").and_then(|v| v.as_str()).unwrap_or("");
    let _ = hash_u64(&["retweets_post", tweet_id]);
    json!({ "data": { "id": tweet_id, "retweeted": true } })
}
