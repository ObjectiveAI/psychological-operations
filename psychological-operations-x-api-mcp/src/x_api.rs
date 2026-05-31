//! `PsychologicalOperationsXApiMcp` — RMCP server wrapping the X v2 API
//! through the workspace's SDK (`psychological_operations_sdk::x::*`).
//!
//! Seven tools: tweet lookup, reply chain traversal, user bio + profile
//! picture, search, and a singular media-attachment opener. All
//! tweet + media handling delegates to the SDK's `x::tweet` module —
//! the MCP file stays thin and never owns its own HTTP client or
//! JSON-walking logic. The cache lives entirely in the SDK.

use std::sync::Arc;

use base64::Engine;
use psychological_operations_sdk::x::http::Http;
use psychological_operations_sdk::x::tweet::{
    self, AttachmentKind, FetchedAttachment, Tweet,
};
use reqwest::Method;
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{
        Content, Implementation, ProtocolVersion, ServerCapabilities,
        ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::Value;

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
    #[schemars(description = "Attachment URL as returned in get_tweet's attachments[].url.")]
    pub url: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RunQueryRequest {
    #[schemars(description = "Raw X v2 search query (e.g. \"from:openai -is:retweet\").")]
    pub query: String,
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
        description = "Fetch a tweet by ID. Returns JSON: { tweet_id, handle, content, attachments: [{ kind, url }, ...] }. Attachments are tiny references — pass an attachment URL plus the tweet_id to open_attachment to retrieve the actual bytes."
    )]
    async fn get_tweet(&self, Parameters(req): Parameters<GetTweetRequest>) -> String {
        match tweet::get_tweet(&self.http, &req.tweet_id, true).await {
            Ok(t) => serde_json::to_string(&t)
                .unwrap_or_else(|e| format!("error: serialize tweet: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "open_attachment",
        description = "Fetch the bytes for one tweet attachment. Returns an MCP image content block for photos, or a text content block carrying a `data:<mime>;base64,...` URI for videos / animated GIFs. The tweet_id is required so the SDK can look the URL up authoritatively against the tweet's media expansion."
    )]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
    ) -> Content {
        let fetched: FetchedAttachment = match tweet::fetch_attachment(
            &self.http,
            &req.tweet_id,
            &req.url,
            true,
        )
        .await
        {
            Ok(a) => a,
            Err(e) => return Content::text(format!("error: {e}")),
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);
        match fetched.kind {
            AttachmentKind::Photo => Content::image(b64, fetched.mime),
            AttachmentKind::Video => {
                Content::text(format!("data:{};base64,{}", fetched.mime, b64))
            }
        }
    }

    #[tool(
        name = "run_query",
        description = "Run an X v2 recent search. Returns a JSON array of canonical tweets (tweet_id, handle, content, attachments)."
    )]
    async fn run_query(&self, Parameters(req): Parameters<RunQueryRequest>) -> String {
        match tweet::search_recent(&self.http, &req.query, true).await {
            Ok(tweets) => serde_json::to_string::<Vec<Tweet>>(&tweets)
                .unwrap_or_else(|e| format!("error: serialize tweets: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }
}

// =====================================================================
// Internal helpers — kept only for the tools that still walk raw Value
// (get_replied_to_id / list_reply_ids / get_bio / get_profile_picture).
// =====================================================================

impl PsychologicalOperationsXApiMcp {
    async fn fetch_tweet_value(&self, tweet_id: &str) -> Result<Value, String> {
        #[derive(Serialize)]
        struct Q<'a> {
            #[serde(rename = "tweet.fields")]
            tweet_fields: &'a str,
            expansions: &'a str,
        }
        let q = Q {
            tweet_fields: "referenced_tweets",
            expansions: "referenced_tweets.id",
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

fn urlencoding_encode(raw: &str) -> String {
    // Tiny inline percent-encoder for path segments.
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
