//! Higher-level helpers for tweet + media handling.
//!
//! Wraps the codegen'd endpoints in a normalised shape and
//! centralises the X-specific "media is expansion-only" + "videos
//! live in `variants`" knowledge so every consumer (MCP / CLI /
//! future crates) sees the same view.
//!
//! Cheap-by-default — [`Tweet::attachments`] carries only
//! `(kind, url)` references, NO bytes / mime / base64. Agents
//! never pay context for media they don't actually look at; bytes
//! are fetched on demand via [`fetch_attachment`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::x::Error;
use crate::x::http::Http;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Photo,
    /// Covers both X's `video` and `animated_gif` media types
    /// (both are served as `video/mp4` variant streams).
    Video,
}

/// A small, agent-safe reference to one media attachment on a
/// tweet. Carries no bytes — agents see only kind + url, fetching
/// the actual payload on demand via [`fetch_attachment`] when (and
/// only when) they decide to look. This keeps tweet objects cheap
/// in agent context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    /// Direct CDN URL — photo URL for photos, the best
    /// (highest-bit-rate) `video/mp4` variant URL for video /
    /// animated_gif.
    pub url: String,
}

/// Canonical tweet shape returned by [`get_tweet`] / [`search_recent`].
/// Strictly small — every field is a `String`, every attachment is
/// a `(kind, url)` reference. No bytes here, ever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tweet {
    pub tweet_id: String,
    pub handle: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
}

/// The fetched-payload form returned by [`fetch_attachment`].
/// Separate type from [`Attachment`] precisely because this one
/// IS large — agents only ever construct or hold one of these
/// after explicitly opening a specific attachment.
#[derive(Debug, Clone)]
pub struct FetchedAttachment {
    pub kind: AttachmentKind,
    pub mime: String,
    pub bytes: Vec<u8>,
}

// =====================================================================
// Public API
// =====================================================================

/// `GET /2/tweets/{id}` with the standard expansions baked in —
/// `expansions=author_id,attachments.media_keys`,
/// `media.fields=url,variants,preview_image_url,type`,
/// `user.fields=username`,
/// `tweet.fields=text,author_id,attachments`. Returns the
/// canonical [`Tweet`] (no byte payloads).
pub async fn get_tweet(
    http: &Http,
    tweet_id: &str,
    cache: bool,
) -> Result<Tweet, Error> {
    let v = fetch_tweet_value(http, tweet_id, cache).await?;
    tweet_from_value(&v, tweet_id).ok_or_else(|| {
        Error::Other(format!("tweet {tweet_id}: response missing `data`"))
    })
}

/// `GET /2/tweets/search/recent` with the same standard
/// expansions. Returns one [`Tweet`] per search hit (still
/// byte-free).
pub async fn search_recent(
    http: &Http,
    query: &str,
    cache: bool,
) -> Result<Vec<Tweet>, Error> {
    let v = search_recent_value(http, query, cache).await?;
    let tweets = v
        .pointer("/data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(tweets.len());
    for t in &tweets {
        let Some(tid) = t.get("id").and_then(Value::as_str) else {
            continue;
        };
        // Wrap each search hit into the same `{data, includes}`
        // shape `/2/tweets/{id}` returns so the canonical
        // extractor can reuse the includes block.
        let wrapped = serde_json::json!({
            "data": t,
            "includes": v.get("includes").cloned().unwrap_or(Value::Null),
        });
        if let Some(tweet) = tweet_from_value(&wrapped, tid) {
            out.push(tweet);
        }
    }
    Ok(out)
}

/// Fetch the bytes for an attachment URL belonging to a known
/// tweet. The `(tweet_id, url)` pair lets us re-fetch the tweet
/// (cache hit) and look the URL up against `includes.media` to
/// determine the media kind + mime authoritatively from X — no
/// URL-extension guessing for videos, no trusting `kind` smuggled
/// in by the caller.
///
/// Errors with `Error::Other` if the URL isn't present in any of
/// the tweet's media entries (photo `m.url` or any
/// `m.variants[].url`).
pub async fn fetch_attachment(
    http: &Http,
    tweet_id: &str,
    url: &str,
    cache: bool,
) -> Result<FetchedAttachment, Error> {
    let v = fetch_tweet_value(http, tweet_id, cache).await?;
    let (kind, mime) = find_attachment_kind(&v, url).ok_or_else(|| {
        Error::Other(format!(
            "attachment URL not on tweet {tweet_id}: {url}"
        ))
    })?;
    let bytes = http.fetch_url(url, cache).await?;
    Ok(FetchedAttachment { kind, mime, bytes })
}

// =====================================================================
// Internals — tweet fetch + JSON walk
// =====================================================================

async fn fetch_tweet_value(
    http: &Http,
    tweet_id: &str,
    cache: bool,
) -> Result<Value, Error> {
    #[derive(Serialize)]
    struct Q<'a> {
        #[serde(rename = "tweet.fields")]
        tweet_fields: &'a str,
        expansions: &'a str,
        #[serde(rename = "media.fields")]
        media_fields: &'a str,
        #[serde(rename = "user.fields")]
        user_fields: &'a str,
    }
    let q = Q {
        tweet_fields: "author_id,attachments,referenced_tweets,text",
        expansions: "author_id,attachments.media_keys,referenced_tweets.id",
        media_fields: "url,variants,preview_image_url,type",
        user_fields: "username",
    };
    let path = format!("tweets/{}", url_encode_segment(tweet_id));
    http.send_with_query::<Value, _>(
        reqwest::Method::GET,
        &path,
        &q,
        cache,
    )
    .await
}

async fn search_recent_value(
    http: &Http,
    query: &str,
    cache: bool,
) -> Result<Value, Error> {
    #[derive(Serialize)]
    struct Q<'a> {
        query: &'a str,
        #[serde(rename = "tweet.fields")]
        tweet_fields: &'a str,
        expansions: &'a str,
        #[serde(rename = "media.fields")]
        media_fields: &'a str,
        #[serde(rename = "user.fields")]
        user_fields: &'a str,
        max_results: u32,
    }
    let q = Q {
        query,
        tweet_fields: "author_id,attachments,text",
        expansions: "author_id,attachments.media_keys",
        media_fields: "url,variants,preview_image_url,type",
        user_fields: "username",
        max_results: 100,
    };
    http.send_with_query::<Value, _>(
        reqwest::Method::GET,
        "tweets/search/recent",
        &q,
        cache,
    )
    .await
}

/// Build a canonical [`Tweet`] from a `/2/tweets/{id}`-shaped
/// Value (`{data: Tweet, includes: Expansions}`).
fn tweet_from_value(v: &Value, tweet_id: &str) -> Option<Tweet> {
    let data = v.get("data")?;
    let content = data
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let author_id = data.get("author_id").and_then(Value::as_str);
    let handle = author_id
        .and_then(|aid| {
            v.pointer("/includes/users")
                .and_then(Value::as_array)
                .and_then(|users| {
                    users.iter().find_map(|u| {
                        (u.get("id").and_then(Value::as_str) == Some(aid))
                            .then(|| u.get("username").and_then(Value::as_str))
                            .flatten()
                            .map(|s| s.to_string())
                    })
                })
        })
        .unwrap_or_default();

    let media_keys: Vec<String> = data
        .pointer("/attachments/media_keys")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|k| k.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let media_objs: Vec<Value> = v
        .pointer("/includes/media")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut attachments = Vec::new();
    for key in &media_keys {
        let Some(m) = media_objs.iter().find(|m| {
            m.get("media_key").and_then(Value::as_str) == Some(key.as_str())
        }) else {
            continue;
        };
        if let Some(a) = attachment_ref_for(m) {
            attachments.push(a);
        }
    }

    Some(Tweet {
        tweet_id: tweet_id.to_string(),
        handle,
        content,
        attachments,
    })
}

/// Translate one media-object Value into an [`Attachment`]
/// reference (kind + canonical URL). For videos / animated_gifs
/// we use the highest-bit-rate `video/mp4` variant; that URL is
/// what `fetch_attachment` later expects.
fn attachment_ref_for(m: &Value) -> Option<Attachment> {
    let kind = m.get("type").and_then(Value::as_str)?;
    match kind {
        "photo" => {
            let url = m.get("url").and_then(Value::as_str)?.to_string();
            Some(Attachment {
                kind: AttachmentKind::Photo,
                url,
            })
        }
        "video" | "animated_gif" => best_video_variant(m).map(|(url, _ct)| {
            Attachment {
                kind: AttachmentKind::Video,
                url,
            }
        }),
        _ => None,
    }
}

/// Filter the media object's `variants` to `video/mp4`, pick the
/// highest `bit_rate`, return `(url, content_type)`. Plumbed
/// content_type lets `fetch_attachment` carry mime through to the
/// caller without re-guessing.
fn best_video_variant(m: &Value) -> Option<(String, String)> {
    let variants = m.get("variants").and_then(Value::as_array)?;
    let mut best: Option<(i64, &str, &str)> = None; // (bit_rate, url, ct)
    for v in variants {
        let ct = v.get("content_type").and_then(Value::as_str).unwrap_or("");
        if ct != "video/mp4" {
            continue;
        }
        let br = v.get("bit_rate").and_then(Value::as_i64).unwrap_or(0);
        let Some(url) = v.get("url").and_then(Value::as_str) else {
            continue;
        };
        if best.map(|(b, _, _)| br > b).unwrap_or(true) {
            best = Some((br, url, ct));
        }
    }
    best.map(|(_, u, ct)| (u.to_string(), ct.to_string()))
}

/// Walk the response's `includes.media` looking for the entry
/// whose URL matches `url`. Returns the discovered
/// `(AttachmentKind, mime)` so `fetch_attachment` can carry a
/// server-authoritative mime back to the caller.
fn find_attachment_kind(
    v: &Value,
    url: &str,
) -> Option<(AttachmentKind, String)> {
    let media = v
        .pointer("/includes/media")
        .and_then(Value::as_array)?;
    for m in media {
        let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "photo" => {
                if m.get("url").and_then(Value::as_str) == Some(url) {
                    return Some((
                        AttachmentKind::Photo,
                        mime_for_photo_url(url).to_string(),
                    ));
                }
            }
            "video" | "animated_gif" => {
                let variants =
                    m.get("variants").and_then(Value::as_array);
                if let Some(arr) = variants {
                    for var in arr {
                        if var.get("url").and_then(Value::as_str) == Some(url)
                        {
                            let ct = var
                                .get("content_type")
                                .and_then(Value::as_str)
                                .unwrap_or("video/mp4")
                                .to_string();
                            return Some((AttachmentKind::Video, ct));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Infer a photo mime from the URL's path-tail extension. X's
/// photo URLs are reliably `.jpg` / `.png` / `.webp` / `.gif`;
/// fallback is jpeg.
fn mime_for_photo_url(url: &str) -> &'static str {
    let no_q = url.split('?').next().unwrap_or(url);
    let no_h = no_q.split('#').next().unwrap_or(no_q);
    let last = no_h.rsplit('/').next().unwrap_or(no_h);
    let ext = last.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

/// Percent-encode a single path segment (everything outside RFC
/// 3986 `unreserved`).
fn url_encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
