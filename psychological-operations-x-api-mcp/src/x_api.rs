//! `PsychologicalOperationsXApiMcp` — RMCP server wrapping the X v2 API
//! through the workspace's SDK (`psychological_operations_sdk::x::*`).
//!
//! Seven tools: tweet lookup, reply chain traversal, user bio + profile
//! picture, search, and a local-attachment opener. Every X v2 call goes
//! through the SDK's `Http` (app-only bearer + SQLite response cache);
//! every `cache: true` so the second hit of the same request stays
//! cheap.
//!
//! Responses from X are deserialised into [`serde_json::Value`] because
//! the codegen'd `Media` struct is missing `url` / `variants` /
//! `preview_image_url` — fields we need to download attachments. The
//! tool bodies walk the Value directly for those fields and only
//! reach into typed shapes where the codegen covers what we need.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use psychological_operations_sdk::x::http::Http;
use reqwest::Method;
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::{Value, json};

const ATTACHMENTS_SUBDIR: &str = "plugins/psychological-operations/x-api-mcp/attachments";

#[derive(Clone)]
pub struct PsychologicalOperationsXApiMcp {
    pub tool_router: ToolRouter<Self>,
    http: Arc<Http>,
    /// Outer `config_base_dir` — attachments live under
    /// `<this>/plugins/psychological-operations/x-api-mcp/attachments/<tweet_id>/`.
    config_base_dir: PathBuf,
}

// `ToolRouter` is hand-implemented Debug-poor; project the relevant
// fields so the macro-generated Debug bound on Parameters is satisfied.
impl std::fmt::Debug for PsychologicalOperationsXApiMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsychologicalOperationsXApiMcp")
            .field("config_base_dir", &self.config_base_dir)
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
pub struct OpenAttachmentsRequest {
    #[schemars(description = "Numeric tweet ID whose attachments you want to open.")]
    pub tweet_id: String,
    #[schemars(description = "Basenames as returned by get_tweet's image_attachments / video_attachments.")]
    pub filenames: Vec<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RunQueryRequest {
    #[schemars(description = "Raw X v2 search query (e.g. \"from:openai -is:retweet\").")]
    pub query: String,
}

/// Returned tweet shape — what every read-side tool serialises into its
/// String result.
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
    pub fn new(http: Arc<Http>, config_base_dir: PathBuf) -> Self {
        Self {
            tool_router: Self::tool_router(),
            http,
            config_base_dir,
        }
    }

    #[tool(
        name = "get_replied_to_id",
        description = "Return the ID of the tweet that the given tweet is replying to, or an empty string when the tweet isn't a reply."
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
        description = "Return the IDs of recent tweets that reply to the given tweet (X v2 search /2/tweets/search/recent with conversation_id), as a JSON array string."
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
        description = "Return the X user's bio (`description` field) for the given handle."
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
        description = "Return the X user's `profile_image_url` for the given handle."
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
        description = "Fetch a tweet by ID, returning JSON with `tweet_id`, `handle`, `content`, `image_attachments`, `video_attachments`. Media URLs are downloaded into the per-tweet attachments directory; the attachment lists are basenames."
    )]
    async fn get_tweet(&self, Parameters(req): Parameters<GetTweetRequest>) -> String {
        match self.fetch_tweet_value(&req.tweet_id).await {
            Ok(v) => match self.canonical_from_value(&v, &req.tweet_id).await {
                Some(t) => serde_json::to_string(&t)
                    .unwrap_or_else(|e| format!("error: serialize tweet: {e}")),
                None => "error: response missing `data`".into(),
            },
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "open_attachments",
        description = "Open each given attachment basename (as returned by get_tweet) with the system default viewer."
    )]
    async fn open_attachments(
        &self,
        Parameters(req): Parameters<OpenAttachmentsRequest>,
    ) -> String {
        let dir = self.attachments_root().join(&req.tweet_id);
        let mut opened: u32 = 0;
        let mut errors = Vec::new();
        for name in &req.filenames {
            let path = dir.join(name);
            match opener::open(&path) {
                Ok(()) => opened += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        if errors.is_empty() {
            format!("opened {opened}")
        } else {
            format!("opened {opened}; errors: {}", errors.join("; "))
        }
    }

    #[tool(
        name = "run_query",
        description = "Run an X v2 search query (X v2 /2/tweets/search/recent). Returns a JSON array of canonical tweet objects (tweet_id, handle, content, image_attachments, video_attachments)."
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
            // Build a Value the canonical extractor can consume — wrap
            // each search-hit tweet into the same `{data, includes}`
            // shape /2/tweets/{id} returns. `includes.users` /
            // `includes.media` carry the full set; the extractor
            // filters by author_id + media_keys.
            let wrapped = json!({ "data": t, "includes": v.get("includes").cloned().unwrap_or(Value::Null) });
            if let Some(c) = self.canonical_from_value(&wrapped, &tweet_id).await {
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
    /// `<config_base_dir>/plugins/psychological-operations/x-api-mcp/attachments/`
    fn attachments_root(&self) -> PathBuf {
        self.config_base_dir.join(ATTACHMENTS_SUBDIR)
    }

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

    /// Build a [`CanonicalTweet`] from a `/2/tweets/{id}`-shaped Value
    /// (`{data: Tweet, includes: Expansions}`). Resolves the author's
    /// handle via `includes.users` keyed by `data.author_id`, and the
    /// media URLs via `includes.media` keyed by
    /// `data.attachments.media_keys`. Media URLs are downloaded into
    /// the per-tweet attachments directory; the returned lists carry
    /// the on-disk basenames.
    async fn canonical_from_value(
        &self,
        v: &Value,
        tweet_id: &str,
    ) -> Option<CanonicalTweet> {
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
            .map(|a| a.clone())
            .unwrap_or_default();

        let mut image_attachments = Vec::new();
        let mut video_attachments = Vec::new();
        let dest_dir = self.attachments_root().join(tweet_id);
        let client = self.http.client.clone();

        for key in &media_keys {
            let m = match media_objs.iter().find(|m| {
                m.get("media_key").and_then(Value::as_str) == Some(key.as_str())
            }) {
                Some(m) => m,
                None => continue,
            };
            let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
            let url_opt = match kind {
                "photo" => m.get("url").and_then(Value::as_str).map(str::to_string),
                "video" | "animated_gif" => best_video_variant(m),
                _ => None,
            };
            let Some(url) = url_opt else { continue };
            match download_media(&client, &dest_dir, &url).await {
                Ok(name) => {
                    if kind == "photo" {
                        image_attachments.push(name);
                    } else {
                        video_attachments.push(name);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "download_media");
                }
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
}

/// Pick the highest-bit-rate `video/mp4` variant from a video / GIF
/// media object's `variants` array. Falls back to the first variant's
/// url when bit_rate is missing on all of them.
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

/// Download `url` to `<dir>/<basename(url)>`. Skips download if the
/// file already exists. Returns the chosen basename.
async fn download_media(
    client: &reqwest::Client,
    dir: &Path,
    url: &str,
) -> std::io::Result<String> {
    let basename = basename_from_url(url);
    let path = dir.join(&basename);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(basename);
    }
    tokio::fs::create_dir_all(dir).await?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| std::io::Error::other(format!("media GET: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(std::io::Error::other(format!("media GET {status}")));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| std::io::Error::other(format!("media body: {e}")))?;
    tokio::fs::write(&path, &bytes).await?;
    Ok(basename)
}

fn basename_from_url(url: &str) -> String {
    // Strip query/fragment, take last path segment.
    let no_q = url.split('?').next().unwrap_or(url);
    let no_h = no_q.split('#').next().unwrap_or(no_q);
    let last = no_h.rsplit('/').next().unwrap_or(no_h);
    if last.is_empty() { "attachment".to_string() } else { last.to_string() }
}

fn urlencoding_encode(raw: &str) -> String {
    // Tiny inline percent-encoder for path segments — same shape the
    // SDK's codegen uses (`urlencoding::encode`), but inlined so we
    // don't pull urlencoding as a direct dep. Encodes everything that
    // isn't unreserved RFC 3986.
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
