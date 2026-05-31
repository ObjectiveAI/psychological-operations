//! `PsychologicalOperationsXApiMcp` — RMCP server wrapping the X v2 API
//! through the workspace SDK (`psychological_operations_sdk::x::*`).
//!
//! Every tool drives the codegen'd `Request`/`Response` types directly
//! via the codegen'd per-endpoint `http` helpers (which already know
//! the URL template, encoding, and the `send_with_query` call). The
//! only custom tweet struct anywhere in this codebase lives here —
//! [`Tweet`] is the small, agent-facing projection that drops the
//! ~30 optional fields the X spec carries on its Tweet schema and
//! keeps the four the agent actually consumes (id, handle, content,
//! attachments).
//!
//! Binary media bytes come from `Client::fetch_url` — the SDK's sole
//! hand-written non-codegen call (twimg has no OpenAPI surface).

use std::sync::Arc;

use base64::Engine;
use psychological_operations_sdk::x::client::Client;
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::tweets::id as tweets_id;
use psychological_operations_sdk::x::tweets::search::recent as tweets_search_recent;
use psychological_operations_sdk::x::types::{
    self as x_types, MediaUnion, TweetId, TweetReferencedTweetsItemType, Variant,
};
use psychological_operations_sdk::x::users::by::username::username as users_by_username;
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

// =====================================================================
// MCP-local types — the *only* custom Tweet struct in the workspace.
// =====================================================================

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum AttachmentKind {
    Photo,
    Video,
    AnimatedGif,
}

#[derive(Debug, Clone, Serialize)]
struct Attachment {
    kind: AttachmentKind,
    url: String,
}

#[derive(Debug, Clone, Serialize)]
struct Tweet {
    tweet_id: String,
    handle: String,
    content: String,
    attachments: Vec<Attachment>,
}

#[derive(Debug, Clone)]
struct FetchedAttachment {
    kind: AttachmentKind,
    mime: String,
    bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct PsychologicalOperationsXApiMcp {
    pub tool_router: ToolRouter<Self>,
    http: Arc<Client>,
}

impl std::fmt::Debug for PsychologicalOperationsXApiMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsychologicalOperationsXApiMcp")
            .finish_non_exhaustive()
    }
}

// =====================================================================
// Per-tool request schemas.
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
    pub fn new(http: Arc<Client>) -> Self {
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
        let creq = tweets_id::get::Request {
            id: TweetId(req.tweet_id.clone()),
            tweet_fields: Some(vec![params::TweetFields::ReferencedTweets]),
            expansions: Some(vec![params::TweetExpansions::ReferencedTweetsId]),
            media_fields: None,
            poll_fields: None,
            user_fields: None,
            place_fields: None,
        };
        let resp = match tweets_id::http::get(&self.http, &creq).await {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let Some(t) = resp.data else { return String::new() };
        let Some(refs) = t.referenced_tweets else { return String::new() };
        refs.into_iter()
            .find(|r| matches!(r.type_, TweetReferencedTweetsItemType::RepliedTo))
            .map(|r| r.id.0)
            .unwrap_or_default()
    }

    #[tool(
        name = "list_reply_ids",
        description = "Return the IDs of recent tweets that reply to the given tweet."
    )]
    async fn list_reply_ids(
        &self,
        Parameters(req): Parameters<ListReplyIdsRequest>,
    ) -> String {
        let creq = tweets_search_recent::get::Request {
            query: format!("conversation_id:{}", req.tweet_id),
            start_time: None,
            end_time: None,
            since_id: None,
            until_id: None,
            max_results: Some(100),
            next_token: None,
            pagination_token: None,
            sort_order: None,
            tweet_fields: Some(vec![params::TweetFields::ReferencedTweets]),
            expansions: None,
            media_fields: None,
            poll_fields: None,
            user_fields: None,
            place_fields: None,
        };
        let resp = match tweets_search_recent::http::get(&self.http, &creq).await {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let target = req.tweet_id;
        let ids: Vec<String> = resp
            .data
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let id = t.id.as_ref()?.0.clone();
                let refs = t.referenced_tweets.as_ref()?;
                refs.iter().any(|r| {
                    matches!(r.type_, TweetReferencedTweetsItemType::RepliedTo)
                        && r.id.0 == target
                }).then_some(id)
            })
            .collect();
        serde_json::to_string(&ids)
            .unwrap_or_else(|e| format!("error: serialize ids: {e}"))
    }

    #[tool(
        name = "get_bio",
        description = "Return the X user's bio."
    )]
    async fn get_bio(&self, Parameters(req): Parameters<GetBioRequest>) -> String {
        let creq = users_by_username::get::Request {
            username: req.handle,
            user_fields: Some(vec![params::UserFields::Description]),
            expansions: None,
            tweet_fields: None,
        };
        match users_by_username::http::get(&self.http, &creq).await {
            Ok(r) => r.data.and_then(|u| u.description).unwrap_or_default(),
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
        let creq = users_by_username::get::Request {
            username: req.handle,
            user_fields: Some(vec![params::UserFields::ProfileImageUrl]),
            expansions: None,
            tweet_fields: None,
        };
        match users_by_username::http::get(&self.http, &creq).await {
            Ok(r) => r
                .data
                .and_then(|u| u.profile_image_url.map(|url| url.to_string()))
                .unwrap_or_default(),
            Err(e) => format!("error: {e}"),
        }
    }

    #[tool(
        name = "get_tweet",
        description = "Fetch a tweet by ID."
    )]
    async fn get_tweet(&self, Parameters(req): Parameters<GetTweetRequest>) -> String {
        let creq = standard_tweet_request(&req.tweet_id);
        let resp = match tweets_id::http::get(&self.http, &creq).await {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let Some(t) = resp.data else {
            return format!("error: tweet {} response had no data", req.tweet_id);
        };
        let projected = project_tweet(&t, resp.includes.as_ref());
        serde_json::to_string(&projected)
            .unwrap_or_else(|e| format!("error: serialize tweet: {e}"))
    }

    #[tool(
        name = "open_attachment",
        description = "Fetch one tweet attachment."
    )]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
    ) -> Content {
        let creq = standard_tweet_request(&req.tweet_id);
        let resp = match tweets_id::http::get(&self.http, &creq).await {
            Ok(r) => r,
            Err(e) => return Content::text(format!("error: {e}")),
        };
        let Some((kind, mime)) =
            lookup_attachment(resp.includes.as_ref(), &req.url)
        else {
            return Content::text(format!(
                "error: attachment URL not on tweet {}: {}",
                req.tweet_id, req.url,
            ));
        };
        let bytes = match self.http.fetch_url(&req.url).await {
            Ok(b) => b,
            Err(e) => return Content::text(format!("error: {e}")),
        };
        let fetched = FetchedAttachment { kind, mime, bytes };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);
        match fetched.kind {
            AttachmentKind::Photo => Content::image(b64, fetched.mime),
            AttachmentKind::Video | AttachmentKind::AnimatedGif => {
                Content::text(format!("data:{};base64,{}", fetched.mime, b64))
            }
        }
    }

    #[tool(
        name = "run_query",
        description = "Run an X v2 recent search. Returns a list of tweets."
    )]
    async fn run_query(&self, Parameters(req): Parameters<RunQueryRequest>) -> String {
        let creq = standard_search_request(req.query);
        let resp = match tweets_search_recent::http::get(&self.http, &creq).await {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let includes = resp.includes.as_ref();
        let projected: Vec<Tweet> = resp
            .data
            .unwrap_or_default()
            .iter()
            .map(|t| project_tweet(t, includes))
            .collect();
        serde_json::to_string(&projected)
            .unwrap_or_else(|e| format!("error: serialize tweets: {e}"))
    }
}

// =====================================================================
// Request builders — bake the standard "tweet + media + author"
// expansion set so every read-side tool sees the same shape.
// =====================================================================

fn standard_tweet_request(tweet_id: &str) -> tweets_id::get::Request {
    tweets_id::get::Request {
        id: TweetId(tweet_id.to_string()),
        tweet_fields: Some(vec![
            params::TweetFields::Attachments,
            params::TweetFields::AuthorId,
            params::TweetFields::Text,
        ]),
        expansions: Some(vec![
            params::TweetExpansions::AttachmentsMediaKeys,
            params::TweetExpansions::AuthorId,
        ]),
        media_fields: Some(vec![
            params::MediaFields::Url,
            params::MediaFields::Variants,
            params::MediaFields::PreviewImageUrl,
            params::MediaFields::Type,
        ]),
        poll_fields: None,
        user_fields: Some(vec![params::UserFields::Username]),
        place_fields: None,
    }
}

fn standard_search_request(query: String) -> tweets_search_recent::get::Request {
    tweets_search_recent::get::Request {
        query,
        start_time: None,
        end_time: None,
        since_id: None,
        until_id: None,
        max_results: Some(100),
        next_token: None,
        pagination_token: None,
        sort_order: None,
        tweet_fields: Some(vec![
            params::TweetFields::Attachments,
            params::TweetFields::AuthorId,
            params::TweetFields::Text,
        ]),
        expansions: Some(vec![
            params::TweetExpansions::AttachmentsMediaKeys,
            params::TweetExpansions::AuthorId,
        ]),
        media_fields: Some(vec![
            params::MediaFields::Url,
            params::MediaFields::Variants,
            params::MediaFields::PreviewImageUrl,
            params::MediaFields::Type,
        ]),
        poll_fields: None,
        user_fields: Some(vec![params::UserFields::Username]),
        place_fields: None,
    }
}

// =====================================================================
// Projection: codegen Tweet + Expansions → agent-facing Tweet.
// =====================================================================

fn project_tweet(t: &x_types::Tweet, includes: Option<&x_types::Expansions>) -> Tweet {
    let tweet_id = t.id.as_ref().map(|i| i.0.clone()).unwrap_or_default();
    let content = t.text.as_ref().map(|tx| tx.0.clone()).unwrap_or_default();
    let handle = resolve_handle(t.author_id.as_ref(), includes);
    let attachments = collect_attachments(t, includes);
    Tweet { tweet_id, handle, content, attachments }
}

fn resolve_handle(
    author_id: Option<&x_types::UserId>,
    includes: Option<&x_types::Expansions>,
) -> String {
    let Some(aid) = author_id else { return String::new() };
    let Some(users) = includes.and_then(|i| i.users.as_ref()) else {
        return String::new();
    };
    users
        .iter()
        .find(|u| u.id.0 == aid.0)
        .map(|u| u.username.0.clone())
        .unwrap_or_default()
}

fn collect_attachments(
    t: &x_types::Tweet,
    includes: Option<&x_types::Expansions>,
) -> Vec<Attachment> {
    let Some(media_keys) = t.attachments.as_ref().and_then(|a| a.media_keys.as_ref()) else {
        return Vec::new();
    };
    let Some(media) = includes.and_then(|i| i.media.as_ref()) else {
        return Vec::new();
    };
    media_keys
        .iter()
        .filter_map(|mk| {
            media
                .iter()
                .find(|m| media_key_matches(m, mk))
                .and_then(attachment_from_media)
        })
        .collect()
}

fn media_key_matches(m: &MediaUnion, mk: &x_types::MediaKey) -> bool {
    let inner_key = match m {
        MediaUnion::Photo(p) => p.flatten_0.media_key.as_ref(),
        MediaUnion::Video(v) => v.flatten_0.media_key.as_ref(),
        MediaUnion::AnimatedGif(a) => a.flatten_0.media_key.as_ref(),
    };
    inner_key.map(|k| k.0 == mk.0).unwrap_or(false)
}

fn attachment_from_media(m: &MediaUnion) -> Option<Attachment> {
    match m {
        MediaUnion::Photo(p) => Some(Attachment {
            kind: AttachmentKind::Photo,
            url: p.url.as_ref()?.to_string(),
        }),
        MediaUnion::Video(v) => best_mp4_variant(v.variants.as_deref()).map(|url| Attachment {
            kind: AttachmentKind::Video,
            url,
        }),
        MediaUnion::AnimatedGif(a) => best_mp4_variant(a.variants.as_deref()).map(|url| Attachment {
            kind: AttachmentKind::AnimatedGif,
            url,
        }),
    }
}

fn best_mp4_variant(variants: Option<&[Variant]>) -> Option<String> {
    variants?
        .iter()
        .filter(|v| v.content_type.as_deref() == Some("video/mp4"))
        .filter_map(|v| {
            let url = v.url.as_ref()?.to_string();
            Some((v.bit_rate.unwrap_or(0), url))
        })
        .max_by_key(|(br, _)| *br)
        .map(|(_, url)| url)
}

fn lookup_attachment(
    includes: Option<&x_types::Expansions>,
    url: &str,
) -> Option<(AttachmentKind, String)> {
    let media = includes?.media.as_ref()?;
    for m in media {
        match m {
            MediaUnion::Photo(p) => {
                if p.url.as_ref().map(|u| u.as_str()) == Some(url) {
                    return Some((AttachmentKind::Photo, mime_for_photo_url(url).to_string()));
                }
            }
            MediaUnion::Video(v) => {
                if let Some(found) = match_variant_url(v.variants.as_deref(), url) {
                    return Some((AttachmentKind::Video, found));
                }
            }
            MediaUnion::AnimatedGif(a) => {
                if let Some(found) = match_variant_url(a.variants.as_deref(), url) {
                    return Some((AttachmentKind::AnimatedGif, found));
                }
            }
        }
    }
    None
}

fn match_variant_url(variants: Option<&[Variant]>, target: &str) -> Option<String> {
    variants?.iter().find_map(|v| {
        if v.url.as_ref().map(|u| u.as_str()) == Some(target) {
            Some(v.content_type.clone().unwrap_or_else(|| "video/mp4".to_string()))
        } else {
            None
        }
    })
}

fn mime_for_photo_url(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
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
