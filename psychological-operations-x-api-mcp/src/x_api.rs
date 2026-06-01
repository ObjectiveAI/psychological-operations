//! `PsychologicalOperationsXApiMcp` — RMCP server wrapping the X v2 API
//! through the workspace SDK (`psychological_operations_sdk::x::*`).
//!
//! Every tool drives the codegen'd `Request`/`Response` types directly
//! via the codegen'd per-endpoint `http` helpers (which already know
//! the URL template, encoding, and the `send_with_query` call). The
//! only custom tweet struct anywhere in this codebase lives here —
//! [`Tweet`] is the small, agent-facing projection that drops the
//! ~30 optional fields the X spec carries on its Tweet schema and
//! keeps the ones the agent actually consumes (id, handle, content,
//! attachments, plus the three optional reference IDs replied_to /
//! quoted / retweeted).
//!
//! Binary media bytes come from `Client::fetch_url` — the SDK's sole
//! hand-written non-codegen call (twimg has no OpenAPI surface).

use std::sync::Arc;

use base64::Engine;
use psychological_operations_sdk::x::client::Client;
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::tweets as tweets_root;
use psychological_operations_sdk::x::tweets::id as tweets_id;
use psychological_operations_sdk::x::tweets::search::recent as tweets_search_recent;
use psychological_operations_sdk::x::types::{
    self as x_types, BookmarkAddRequest, MediaUnion, TweetCreateRequest,
    TweetCreateRequestReply, TweetId, TweetReferencedTweetsItemType, TweetText,
    UserIdMatchesAuthenticatedUser, UsersLikesCreateRequest,
    UsersRetweetsCreateRequest, Variant,
};
use psychological_operations_sdk::x::users::by::username::username as users_by_username;
use psychological_operations_sdk::x::users::id::bookmarks as users_id_bookmarks;
use psychological_operations_sdk::x::users::id::likes as users_id_likes;
use psychological_operations_sdk::x::users::id::retweets as users_id_retweets;
use psychological_operations_sdk::x::users::me as users_me;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::ToolCallContext,
    handler::server::wrapper::Parameters,
    model::{
        CallToolRequestParams, CallToolResult, Content, Implementation,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerInfo, Tool,
    },
    schemars,
    service::RequestContext,
    tool, tool_router,
};
use serde::Serialize;

use crate::Mode;

/// Tools only registered + callable when the server is in
/// `Mode::Full`. All mutations on X.
const FULL_ONLY_TOOLS: &[&str] = &[
    "post_tweet",
    "reply_to_tweet",
    "quote_tweet",
    "like",
    "retweet",
    "bookmark",
];

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
    id: String,
    handle: String,
    content: String,
    attachments: Vec<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replied_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quoted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retweeted: Option<String>,
    reply_count: i32,
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
    mode: Mode,
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoamiRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PostTweetRequest {
    #[schemars(description = "Body text of the new tweet.")]
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReplyToTweetRequest {
    #[schemars(description = "Body text of the reply.")]
    pub text: String,
    #[schemars(description = "Numeric ID of the tweet being replied to.")]
    pub in_reply_to_tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QuoteTweetRequest {
    #[schemars(description = "Body text wrapped around the quote.")]
    pub text: String,
    #[schemars(description = "Numeric ID of the tweet being quoted.")]
    pub quote_tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct LikeRequest {
    #[schemars(description = "Numeric ID of the tweet to like.")]
    pub tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RetweetRequest {
    #[schemars(description = "Numeric ID of the tweet to retweet.")]
    pub tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BookmarkRequest {
    #[schemars(description = "Numeric ID of the tweet to bookmark.")]
    pub tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetBookmarksRequest {}

// =====================================================================
// Tool impls
// =====================================================================

#[tool_router]
impl PsychologicalOperationsXApiMcp {
    pub fn new(http: Arc<Client>, mode: Mode) -> Self {
        Self {
            tool_router: Self::tool_router(),
            http,
            mode,
        }
    }

    /// `true` when this tool is registered but should not be listed
    /// or callable in the current mode.
    fn is_hidden(&self, tool_name: &str) -> bool {
        matches!(self.mode, Mode::Readonly) && FULL_ONLY_TOOLS.contains(&tool_name)
    }

    /// Resolve the authenticated user's numeric id via `/users/me`.
    /// Used by the engagement tools (like / retweet / bookmark)
    /// that need the acting user id in the URL path.
    async fn resolve_self_user_id(&self) -> Result<String, ErrorData> {
        let req = users_me::get::Request {
            user_fields: None,
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_me::http::get(&self.http, &req)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/me: {e}"), None))?;
        let user = resp.data.ok_or_else(|| {
            ErrorData::internal_error("users/me had no data".to_string(), None)
        })?;
        Ok(user.id.0)
    }

    #[tool(
        name = "get_replies",
        description = "Fetch recent replies to a tweet."
    )]
    async fn list_reply_ids(
        &self,
        Parameters(req): Parameters<ListReplyIdsRequest>,
    ) -> Result<String, ErrorData> {
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
        let resp = tweets_search_recent::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("search: {e}"), None))?;
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
            .map_err(|e| ErrorData::internal_error(format!("serialize ids: {e}"), None))
    }

    #[tool(
        name = "get_bio",
        description = "Fetch an X user's bio."
    )]
    async fn get_bio(
        &self,
        Parameters(req): Parameters<GetBioRequest>,
    ) -> Result<String, ErrorData> {
        let creq = users_by_username::get::Request {
            username: req.handle,
            user_fields: Some(vec![params::UserFields::Description]),
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_by_username::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/by/username: {e}"), None))?;
        Ok(resp.data.and_then(|u| u.description).unwrap_or_default())
    }

    #[tool(
        name = "get_profile_picture",
        description = "Fetch an X user's profile picture."
    )]
    async fn get_profile_picture(
        &self,
        Parameters(req): Parameters<GetProfilePictureRequest>,
    ) -> Result<String, ErrorData> {
        let creq = users_by_username::get::Request {
            username: req.handle,
            user_fields: Some(vec![params::UserFields::ProfileImageUrl]),
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_by_username::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/by/username: {e}"), None))?;
        Ok(resp
            .data
            .and_then(|u| u.profile_image_url.map(|url| url.to_string()))
            .unwrap_or_default())
    }

    #[tool(
        name = "get_tweet",
        description = "Fetch a tweet."
    )]
    async fn get_tweet(
        &self,
        Parameters(req): Parameters<GetTweetRequest>,
    ) -> Result<String, ErrorData> {
        let creq = standard_tweet_request(&req.tweet_id);
        let resp = tweets_id::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("tweets/{{id}}: {e}"), None))?;
        let t = resp.data.ok_or_else(|| {
            ErrorData::internal_error(
                format!("tweet {} response had no data", req.tweet_id),
                None,
            )
        })?;
        let projected = project_tweet(&t, resp.includes.as_ref());
        serde_json::to_string(&projected)
            .map_err(|e| ErrorData::internal_error(format!("serialize tweet: {e}"), None))
    }

    #[tool(
        name = "open_attachment",
        description = "Fetch an attachment."
    )]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
    ) -> Result<Content, ErrorData> {
        let creq = standard_tweet_request(&req.tweet_id);
        let resp = tweets_id::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("tweets/{{id}}: {e}"), None))?;
        let (kind, mime) = lookup_attachment(resp.includes.as_ref(), &req.url)
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!(
                        "attachment URL not on tweet {}: {}",
                        req.tweet_id, req.url,
                    ),
                    None,
                )
            })?;
        let bytes = self
            .http
            .fetch_url(&req.url)
            .await
            .map_err(|e| ErrorData::internal_error(format!("fetch_url: {e}"), None))?;
        let fetched = FetchedAttachment { kind, mime, bytes };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);
        Ok(match fetched.kind {
            AttachmentKind::Photo => Content::image(b64, fetched.mime),
            AttachmentKind::Video | AttachmentKind::AnimatedGif => {
                Content::text(format!("data:{};base64,{}", fetched.mime, b64))
            }
        })
    }

    #[tool(
        name = "run_query",
        description = "Run an X v2 recent search."
    )]
    async fn run_query(
        &self,
        Parameters(req): Parameters<RunQueryRequest>,
    ) -> Result<String, ErrorData> {
        let creq = standard_search_request(req.query);
        let resp = tweets_search_recent::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("search: {e}"), None))?;
        let includes = resp.includes.as_ref();
        let projected: Vec<Tweet> = resp
            .data
            .unwrap_or_default()
            .iter()
            .map(|t| project_tweet(t, includes))
            .collect();
        serde_json::to_string(&projected)
            .map_err(|e| ErrorData::internal_error(format!("serialize tweets: {e}"), None))
    }

    #[tool(
        name = "whoami",
        description = "Fetch your own X account's handle (@username)."
    )]
    async fn whoami(
        &self,
        Parameters(_req): Parameters<WhoamiRequest>,
    ) -> Result<String, ErrorData> {
        let req = users_me::get::Request {
            user_fields: Some(vec![params::UserFields::Username]),
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_me::http::get(&self.http, &req)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/me: {e}"), None))?;
        Ok(resp.data.map(|u| u.username.0).unwrap_or_default())
    }

    #[tool(
        name = "post",
        description = "Post a new tweet."
    )]
    async fn post_tweet(
        &self,
        Parameters(req): Parameters<PostTweetRequest>,
    ) -> Result<String, ErrorData> {
        let body = TweetCreateRequest {
            text: Some(TweetText(req.text)),
            ..empty_tweet_create_request()
        };
        send_create_tweet(&self.http, body).await
    }

    #[tool(
        name = "reply",
        description = "Reply to a tweet."
    )]
    async fn reply_to_tweet(
        &self,
        Parameters(req): Parameters<ReplyToTweetRequest>,
    ) -> Result<String, ErrorData> {
        let body = TweetCreateRequest {
            text: Some(TweetText(req.text)),
            reply: Some(TweetCreateRequestReply {
                in_reply_to_tweet_id: TweetId(req.in_reply_to_tweet_id),
                auto_populate_reply_metadata: None,
                exclude_reply_user_ids: None,
            }),
            ..empty_tweet_create_request()
        };
        send_create_tweet(&self.http, body).await
    }

    #[tool(
        name = "quote",
        description = "Quote a tweet."
    )]
    async fn quote_tweet(
        &self,
        Parameters(req): Parameters<QuoteTweetRequest>,
    ) -> Result<String, ErrorData> {
        let body = TweetCreateRequest {
            text: Some(TweetText(req.text)),
            quote_tweet_id: Some(TweetId(req.quote_tweet_id)),
            ..empty_tweet_create_request()
        };
        send_create_tweet(&self.http, body).await
    }

    #[tool(
        name = "like",
        description = "Like a tweet."
    )]
    async fn like(
        &self,
        Parameters(req): Parameters<LikeRequest>,
    ) -> Result<String, ErrorData> {
        let user_id = self.resolve_self_user_id().await?;
        let creq = users_id_likes::post::Request {
            id: UserIdMatchesAuthenticatedUser(user_id),
            body: Some(UsersLikesCreateRequest {
                tweet_id: TweetId(req.tweet_id),
            }),
        };
        let resp = users_id_likes::http::post(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("likes: {e}"), None))?;
        serde_json::to_string(&resp.data)
            .map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))
    }

    #[tool(
        name = "retweet",
        description = "Retweet a tweet."
    )]
    async fn retweet(
        &self,
        Parameters(req): Parameters<RetweetRequest>,
    ) -> Result<String, ErrorData> {
        let user_id = self.resolve_self_user_id().await?;
        let creq = users_id_retweets::post::Request {
            id: UserIdMatchesAuthenticatedUser(user_id),
            body: Some(UsersRetweetsCreateRequest {
                tweet_id: TweetId(req.tweet_id),
            }),
        };
        let resp = users_id_retweets::http::post(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("retweets: {e}"), None))?;
        serde_json::to_string(&resp.data)
            .map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))
    }

    #[tool(
        name = "bookmark",
        description = "Bookmark a tweet."
    )]
    async fn bookmark(
        &self,
        Parameters(req): Parameters<BookmarkRequest>,
    ) -> Result<String, ErrorData> {
        let user_id = self.resolve_self_user_id().await?;
        let creq = users_id_bookmarks::post::Request {
            id: UserIdMatchesAuthenticatedUser(user_id),
            body: BookmarkAddRequest {
                tweet_id: TweetId(req.tweet_id),
            },
        };
        let resp = users_id_bookmarks::http::post(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("bookmarks: {e}"), None))?;
        serde_json::to_string(&resp.data)
            .map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))
    }

    #[tool(
        name = "get_bookmarks",
        description = "Fetch your bookmarked tweets."
    )]
    async fn get_bookmarks(
        &self,
        Parameters(_req): Parameters<GetBookmarksRequest>,
    ) -> Result<String, ErrorData> {
        let user_id = self.resolve_self_user_id().await?;
        let creq = users_id_bookmarks::get::Request {
            id: UserIdMatchesAuthenticatedUser(user_id),
            max_results: Some(100),
            pagination_token: None,
            tweet_fields: Some(vec![
                params::TweetFields::Attachments,
                params::TweetFields::AuthorId,
                params::TweetFields::PublicMetrics,
                params::TweetFields::ReferencedTweets,
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
        };
        let resp = users_id_bookmarks::http::get(&self.http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("bookmarks: {e}"), None))?;
        let includes = resp.includes.as_ref();
        let projected: Vec<Tweet> = resp
            .data
            .unwrap_or_default()
            .iter()
            .map(|t| project_tweet(t, includes))
            .collect();
        serde_json::to_string(&projected)
            .map_err(|e| ErrorData::internal_error(format!("serialize tweets: {e}"), None))
    }
}

/// Empty-init `TweetCreateRequest` (all fields None / default) so
/// tool bodies can `..empty_tweet_create_request()` and only set the
/// fields they care about.
fn empty_tweet_create_request() -> TweetCreateRequest {
    TweetCreateRequest {
        card_uri: None,
        community_id: None,
        direct_message_deep_link: None,
        edit_options: None,
        for_super_followers_only: None,
        geo: None,
        made_with_ai: None,
        media: None,
        nullcast: None,
        paid_partnership: None,
        poll: None,
        quote_tweet_id: None,
        reply: None,
        reply_settings: None,
        share_with_followers: None,
        text: None,
    }
}

/// Shared `POST /2/tweets` plumbing for post / reply / quote.
/// Returns the serialized `data` block on success (the agent gets
/// the new tweet id + text back).
async fn send_create_tweet(
    http: &Client,
    body: TweetCreateRequest,
) -> Result<String, ErrorData> {
    let req = tweets_root::post::Request { body };
    let resp = tweets_root::http::post(http, &req)
        .await
        .map_err(|e| ErrorData::internal_error(format!("tweets: {e}"), None))?;
    serde_json::to_string(&resp.data)
        .map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))
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
            params::TweetFields::PublicMetrics,
            params::TweetFields::ReferencedTweets,
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
            params::TweetFields::PublicMetrics,
            params::TweetFields::ReferencedTweets,
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
    let id = t.id.as_ref().map(|i| i.0.clone()).unwrap_or_default();
    let content = t.text.as_ref().map(|tx| tx.0.clone()).unwrap_or_default();
    let handle = resolve_handle(t.author_id.as_ref(), includes);
    let attachments = collect_attachments(t, includes);

    let (mut replied_to, mut quoted, mut retweeted) = (None, None, None);
    if let Some(refs) = t.referenced_tweets.as_ref() {
        for r in refs {
            match r.type_ {
                TweetReferencedTweetsItemType::RepliedTo => replied_to = Some(r.id.0.clone()),
                TweetReferencedTweetsItemType::Quoted    => quoted     = Some(r.id.0.clone()),
                TweetReferencedTweetsItemType::Retweeted => retweeted  = Some(r.id.0.clone()),
            }
        }
    }

    // `public_metrics` itself is Option (None when not requested by
    // tweet_fields). `reply_count` is spec-required when present;
    // default to 0 if the whole object is missing.
    let reply_count = t.public_metrics.as_ref().map(|m| m.reply_count).unwrap_or(0);

    Tweet {
        id, handle, content, attachments,
        replied_to, quoted, retweeted,
        reply_count,
    }
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
        MediaUnion::Video(v) => best_video_variant(v.variants.as_deref()).map(|url| Attachment {
            kind: AttachmentKind::Video,
            url,
        }),
        MediaUnion::AnimatedGif(a) => best_video_variant(a.variants.as_deref()).map(|url| Attachment {
            kind: AttachmentKind::AnimatedGif,
            url,
        }),
    }
}

/// Pick a playable video variant for this media — preferring
/// the lowest-bit-rate `video/*` rendition X served (trades
/// quality for transfer size; `open_attachment`'s base64 payload
/// back to the agent is dominated by raw bytes). If none of the
/// video variants carry a `bit_rate`, falls back to the first
/// video variant with a URL. Returns `None` when no `video/*`
/// variant exists at all — `attachment_from_media` then returns
/// `None` and `collect_attachments`'s `filter_map` drops the
/// attachment from the agent-facing Tweet.
///
/// The `video/` filter excludes `application/x-mpegURL` HLS
/// playlists (X serves those alongside the real video bytes but
/// they're text manifests, not self-contained playable bytes).
fn best_video_variant(variants: Option<&[Variant]>) -> Option<String> {
    let variants = variants?;
    let is_video = |v: &&Variant| {
        v.content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with("video/"))
    };

    // 1. Lowest-bit-rate among video variants that have a bit_rate.
    let lowest = variants
        .iter()
        .filter(is_video)
        .filter_map(|v| {
            let url = v.url.as_ref()?.to_string();
            let bit_rate = v.bit_rate?;
            Some((bit_rate, url))
        })
        .min_by_key(|(br, _)| *br)
        .map(|(_, url)| url);
    if lowest.is_some() {
        return lowest;
    }

    // 2. Fallback — first video variant with a URL (no bit_rate).
    variants
        .iter()
        .filter(is_video)
        .find_map(|v| v.url.as_ref().map(|u| u.to_string()))
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

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools: Vec<Tool> = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|t| !self.is_hidden(&t.name))
            .collect();
        Ok(ListToolsResult { tools, next_cursor: None })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if self.is_hidden(&request.name) {
            return Err(ErrorData::invalid_params(
                format!("tool '{}' is not available in readonly mode", request.name),
                None,
            ));
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if self.is_hidden(name) {
            None
        } else {
            self.tool_router.get(name).cloned()
        }
    }
}
