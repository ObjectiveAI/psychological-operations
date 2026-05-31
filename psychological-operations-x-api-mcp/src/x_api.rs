//! `PsychologicalOperationsXApiMcp` — RMCP server wrapping the X v2 API
//! through the workspace's SDK (`psychological_operations_sdk::x::*`).
//!
//! Seven tools: tweet lookup, reply chain traversal, user bio + profile
//! picture, search, and a singular media-attachment opener. Every
//! external request routes through the SDK's `Http` (app-only bearer +
//! SQLite response cache) — including the media fetch via
//! [`Http::fetch_url`]. The MCP server owns no parallel HTTP client and
//! no on-disk attachment cache; the SDK's existing cache is the only
//! storage layer.
//!
//! Responses from X are deserialised into [`serde_json::Value`] because
//! the codegen'd `Media` struct is missing `url` / `variants` /
//! `preview_image_url` — fields we need to extract attachment URLs.
//! The tool bodies walk the Value directly for those fields.

use std::sync::Arc;

use base64::Engine;
use psychological_operations_sdk::x::http::Http;
use reqwest::Method;
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{
        Content, Implementation, ProtocolVersion, ResourceContents,
        ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct PsychologicalOperationsXApiMcp {
    pub tool_router: ToolRouter<Self>,
    http: Arc<Http>,
}

impl std::fmt::Debug for PsychologicalOperationsXApiMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsychologicalOperationsXApiMcp")
            .finish_non_exhaustive()
    }
}

// =====================================================================
// Per-tool request schemas (RMCP turns these into the tool input JSON
// Schema via schemars).
// =====================================================================

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetRepliedToIdRequest {
    #[schemars(description = "Numeric ID of the tweet whose reply target you want.")]
    pub tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListReplyIdsRequest {
    #[schemars(description = "Numeric ID of the tweet whose replies you want.")]
    pub tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetBioRequest {
    #[schemars(description = "X handle without the leading @.")]
    pub handle: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetProfilePictureRequest {
    #[schemars(description = "X handle without the leading @.")]
    pub handle: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetTweetRequest {
    #[schemars(description = "Numeric tweet ID.")]
    pub tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenAttachmentRequest {
    #[schemars(description = "Numeric tweet ID the attachment belongs to.")]
    pub tweet_id: String,
    #[schemars(description = "Media key as returned in get_tweet's image_attachments / video_attachments.")]
    pub media_key: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RunQueryRequest {
    #[schemars(description = "Raw X v2 search query (e.g. \"from:openai -is:retweet\").")]
    pub query: String,
}

/// Returned tweet shape — what every read-side tool serialises into
/// its String result. `image_attachments` / `video_attachments` are
/// lists of X's stable opaque media keys (e.g. `"3_1234567890"`); the
/// LLM passes one back to `open_attachment` to retrieve the bytes.
#[derive(Serialize)]
struct CanonicalTweet {
    tweet_id: String,
    handle: String,
    content: String,
    image_attachments: Vec<String>,
    video_attachments: Vec<String>,
}

// =====================================================================
// Tool impls
// =====================================================================

#[tool_router]
impl PsychologicalOperationsXApiMcp {
    pub fn new(http: Arc<Http>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            http,
        }
    }

    #[tool(
        name = "get_replied_to_id",
        description = "Return the ID of the tweet that the given tweet is replying to."
    )]
    async fn get_replied_to_id(
        &self,
        Parameters(req): Parameters<GetRepliedToIdRequest>,
    ) -> String {
        match self.fetch_tweet_value(&req.tweet_id).await {
            Ok(v) => {
                let referenced = v
                    .pointer("/data/referenced_tweets")
                    .and_then(Value::as_array);
                match referenced {
                    Some(items) => items
                        .iter()
                        .find(|item| {
                            item.get("type").and_then(Value::as_str) == Some("replied_to")
                        })
                        .and_then(|item| item.get("id").and_then(Value::as_str))
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    None => String::new(),
                }
            }
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "list_reply_ids",
        description = "Return the IDs of recent tweets that reply to the given tweet."
    )]
    async fn list_reply_ids(
        &self,
        Parameters(req): Parameters<ListReplyIdsRequest>,
    ) -> String {
        let query = format!("conversation_id:{}", req.tweet_id);
        #[derive(Serialize)]
        struct Q<'a> {
            query: &'a str,
            #[serde(rename = "tweet.fields")]
            tweet_fields: &'a str,
            max_results: u32,
        }
        let q = Q {
            query: &query,
            tweet_fields: "referenced_tweets",
            max_results: 100,
        };
        let raw: Result<Value, _> = self
            .http
            .send_with_query::<Value, _>(Method::GET, "tweets/search/recent", &q, true)
            .await;
        match raw {
            Ok(v) => {
                let mut ids = Vec::new();
                if let Some(arr) = v.pointer("/data").and_then(Value::as_array) {
                    for t in arr {
                        let is_reply_to_us = t
                            .get("referenced_tweets")
                            .and_then(Value::as_array)
                            .map(|refs| {
                                refs.iter().any(|r| {
                                    r.get("type").and_then(Value::as_str) == Some("replied_to")
                                        && r.get("id").and_then(Value::as_str)
                                            == Some(req.tweet_id.as_str())
                                })
                            })
                            .unwrap_or(false);
                        if is_reply_to_us {
                            if let Some(id) = t.get("id").and_then(Value::as_str) {
                                ids.push(id.to_string());
                            }
                        }
                    }
                }
                serde_json::to_string(&ids)
                    .unwrap_or_else(|e| format!("error: serialize ids: {e}"))
            }
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "get_bio",
        description = "Return the X user's bio."
    )]
    async fn get_bio(&self, Parameters(req): Parameters<GetBioRequest>) -> String {
        match self.fetch_user_value(&req.handle, "description").await {
            Ok(v) => v
                .pointer("/data/description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "get_profile_picture",
        description = "Return the X user's profile picture URL."
    )]
    async fn get_profile_picture(
        &self,
        Parameters(req): Parameters<GetProfilePictureRequest>,
    ) -> String {
        match self.fetch_user_value(&req.handle, "profile_image_url").await {
            Ok(v) => v
                .pointer("/data/profile_image_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "get_tweet",
        description = "Fetch a tweet by ID. Returns JSON with tweet_id, handle, content, image_attachments, video_attachments. Attachment lists carry X media keys; pass one to open_attachment to get the bytes."
    )]
    async fn get_tweet(&self, Parameters(req): Parameters<GetTweetRequest>) -> String {
        match self.fetch_tweet_value(&req.tweet_id).await {
            Ok(v) => match canonical_from_value(&v, &req.tweet_id) {
                Some(t) => serde_json::to_string(&t)
                    .unwrap_or_else(|e| format!("error: serialize tweet: {e}")),
                None => "error: response missing `data`".into(),
            },
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "open_attachment",
        description = "Re-fetch the tweet (cache hit, instant), resolve the media URL for the given media_key, fetch the bytes via the SDK cache, and return them inline as MCP image content (photos) or an embedded resource with video/mp4 mime (videos / animated GIFs)."
    )]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
    ) -> Content {
        let v = match self.fetch_tweet_value(&req.tweet_id).await {
            Ok(v) => v,
            Err(e) => return Content::text(format!("error: {e}")),
        };
        let media_objs = v
            .pointer("/includes/media")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let m = match media_objs.iter().find(|m| {
            m.get("media_key").and_then(Value::as_str) == Some(req.media_key.as_str())
        }) {
            Some(m) => m,
            None => {
                return Content::text(format!(
                    "error: media_key {} not found on tweet {}",
                    req.media_key, req.tweet_id,
                ));
            }
        };
        let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
        let url_opt = match kind {
            "photo" => m.get("url").and_then(Value::as_str).map(str::to_string),
            "video" | "animated_gif" => best_video_variant(m),
            other => {
                return Content::text(format!(
                    "error: unsupported media type \"{other}\" for media_key {}",
                    req.media_key,
                ));
            }
        };
        let url = match url_opt {
            Some(u) => u,
            None => {
                return Content::text(format!(
                    "error: no playable URL for media_key {}",
                    req.media_key,
                ));
            }
        };
        let bytes = match self.http.fetch_url(&url, true).await {
            Ok(b) => b,
            Err(e) => return Content::text(format!("error: fetch_url: {e}")),
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        match kind {
            "photo" => Content::image(b64, mime_for_photo_url(&url)),
            // video | animated_gif — MCP has no native Video content
            // variant. Embedded-resource with video/mp4 mime is the
            // canonical MCP shape for "client should handle binary".
            _ => Content::resource(ResourceContents::BlobResourceContents {
                uri: url,
                mime_type: Some("video/mp4".into()),
                blob: b64,
                meta: None,
            }),
        }
    }

    #[tool(
        name = "run_query",
        description = "Run an X v2 search query (X v2 /2/tweets/search/recent)."
    )]
    async fn run_query(&self, Parameters(req): Parameters<RunQueryRequest>) -> String {
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
            query: &req.query,
            tweet_fields: "author_id,attachments,text",
            expansions: "author_id,attachments.media_keys",
            media_fields: "url,variants,preview_image_url,type",
            user_fields: "username",
            max_results: 100,
        };
        let raw: Result<Value, _> = self
            .http
            .send_with_query::<Value, _>(Method::GET, "tweets/search/recent", &q, true)
            .await;
        let v = match raw {
            Ok(v) => v,
            Err(e) => return format!("error: {e}"),
        };
        let tweets = match v.pointer("/data").and_then(Value::as_array) {
            Some(a) => a.clone(),
            None => return "[]".into(),
        };
        let mut out: Vec<CanonicalTweet> = Vec::with_capacity(tweets.len());
        for t in &tweets {
            let tweet_id = match t.get("id").and_then(Value::as_str) {
                Some(s) => s.to_string(),
                None => continue,
            };
            // Wrap each search-hit tweet into the same `{data, includes}`
            // shape /2/tweets/{id} returns so the canonical extractor
            // can reuse the includes block.
            let wrapped = json!({
                "data": t,
                "includes": v.get("includes").cloned().unwrap_or(Value::Null),
            });
            if let Some(c) = canonical_from_value(&wrapped, &tweet_id) {
                out.push(c);
            }
        }
        serde_json::to_string(&out)
            .unwrap_or_else(|e| format!("error: serialize tweets: {e}"))
    }
}

// =====================================================================
// Internal helpers
// =====================================================================

impl PsychologicalOperationsXApiMcp {
    async fn fetch_tweet_value(&self, tweet_id: &str) -> Result<Value, String> {
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
        let path = format!("tweets/{}", urlencoding_encode(tweet_id));
        self.http
            .send_with_query::<Value, _>(Method::GET, &path, &q, true)
            .await
            .map_err(|e| e.to_string())
    }

    async fn fetch_user_value(&self, handle: &str, fields: &str) -> Result<Value, String> {
        #[derive(Serialize)]
        struct Q<'a> {
            #[serde(rename = "user.fields")]
            user_fields: &'a str,
        }
        let q = Q { user_fields: fields };
        let path = format!("users/by/username/{}", urlencoding_encode(handle));
        self.http
            .send_with_query::<Value, _>(Method::GET, &path, &q, true)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Build a [`CanonicalTweet`] from a `/2/tweets/{id}`-shaped Value
/// (`{data: Tweet, includes: Expansions}`). Resolves the author's
/// handle via `includes.users` keyed by `data.author_id`, and surfaces
/// each attachment's stable `media_key` (no download — the LLM passes
/// the key back to `open_attachment` to retrieve bytes).
fn canonical_from_value(v: &Value, tweet_id: &str) -> Option<CanonicalTweet> {
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

    let mut image_attachments = Vec::new();
    let mut video_attachments = Vec::new();
    for key in &media_keys {
        let Some(m) = media_objs.iter().find(|m| {
            m.get("media_key").and_then(Value::as_str) == Some(key.as_str())
        }) else {
            continue;
        };
        let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "photo" => image_attachments.push(key.clone()),
            "video" | "animated_gif" => video_attachments.push(key.clone()),
            _ => {}
        }
    }

    Some(CanonicalTweet {
        tweet_id: tweet_id.to_string(),
        handle,
        content,
        image_attachments,
        video_attachments,
    })
}

/// Pick the highest-bit-rate `video/mp4` variant from a video / GIF
/// media object's `variants` array.
fn best_video_variant(m: &Value) -> Option<String> {
    let variants = m.get("variants").and_then(Value::as_array)?;
    let mut best: Option<(i64, &str)> = None;
    for v in variants {
        let ct = v.get("content_type").and_then(Value::as_str).unwrap_or("");
        if ct != "video/mp4" {
            continue;
        }
        let br = v.get("bit_rate").and_then(Value::as_i64).unwrap_or(0);
        let url = match v.get("url").and_then(Value::as_str) {
            Some(u) => u,
            None => continue,
        };
        if best.map(|(b, _)| br > b).unwrap_or(true) {
            best = Some((br, url));
        }
    }
    best.map(|(_, u)| u.to_string())
}

/// Infer a photo mime from the URL's path-tail extension. X's photo
/// URLs are reliably `.jpg` / `.png` / `.webp`; fallback is jpeg.
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

fn urlencoding_encode(raw: &str) -> String {
    // Tiny inline percent-encoder for path segments — same shape the
    // SDK's codegen uses (`urlencoding::encode`), but inlined so we
    // don't pull urlencoding as a direct dep.
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

#[tool_handler]
impl ServerHandler for PsychologicalOperationsXApiMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "psychological-operations-x-api".into(),
                title: None,
                version: env!("CARGO_PKG_VERSION").into(),
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }
}
