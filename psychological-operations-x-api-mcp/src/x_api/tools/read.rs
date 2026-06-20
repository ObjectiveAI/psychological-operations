//! Read tools — pure-GET endpoints. `get_bookmarks` lives here too
//! even though its endpoint shares a URL prefix with the `bookmark`
//! write: the GET is a read.
//!
//! Each tool acts as the session's `tag` (from the
//! `X-OBJECTIVEAI-ARGUMENTS` header) — the client is built bare and the
//! persona is `AuthMode::Agent(tag)`; mode-gating and the per-tag quota
//! gate run centrally in `call_tool` before dispatch.
//!
//! Each body runs inside [`finish`] so failures classify (see
//! [`super::super::tool_error`]): authorization-resolution and infra
//! errors surface as protocol errors; the authorized request's own
//! failures and bad agent inputs surface as `is_error` tool results.

use base64::Engine;
use psychological_operations_sdk::x::client::AuthMode;
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::tweets::id as tweets_id;
use psychological_operations_sdk::x::tweets::search::recent as tweets_search_recent;
use psychological_operations_sdk::x::types::{
    PaginationToken32, PaginationToken36, TweetReferencedTweetsItemType, User,
    UserIdMatchesAuthenticatedUser,
};
use psychological_operations_sdk::x::users::by::username::username as users_by_username;
use psychological_operations_sdk::x::users::id::bookmarks as users_id_bookmarks;
use psychological_operations_sdk::x::users::id::followers as users_id_followers;
use psychological_operations_sdk::x::users::id::following as users_id_following;
use psychological_operations_sdk::x::users::me as users_me;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::builders::{
    resolve_handle_user_id, resolve_self_user_id, standard_search_request, standard_tweet_request,
};
use super::super::model::{AttachmentKind, FetchedAttachment, Tweet};
use super::super::projection::{lookup_attachment, project_tweet};
use super::super::tool_error::{ToolError, finish};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetRepliesRequest {
    #[schemars(description = "Numeric ID of the tweet whose replies you want.")]
    pub tweet_id: String,
    #[schemars(description = "How many replies to return (after skipping `offset`).")]
    pub count: u32,
    #[schemars(description = "How many replies to skip from the start.")]
    pub offset: u32,
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
    #[schemars(description = "How many tweets to return (after skipping `offset`).")]
    pub count: u32,
    #[schemars(description = "How many tweets to skip from the start.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoamiRequest {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetBookmarksRequest {
    #[schemars(description = "How many bookmarks to return (after skipping `offset`).")]
    pub count: u32,
    #[schemars(description = "How many bookmarks to skip from the start.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListFollowingRequest {
    #[schemars(description = "X handle without the leading @ whose following list you want.")]
    pub handle: String,
    #[schemars(description = "How many accounts to return (after skipping `offset`).")]
    pub count: u32,
    #[schemars(description = "How many accounts to skip from the start of the list.")]
    pub offset: u32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListFollowersRequest {
    #[schemars(description = "X handle without the leading @ whose followers list you want.")]
    pub handle: String,
    #[schemars(description = "How many accounts to return (after skipping `offset`).")]
    pub count: u32,
    #[schemars(description = "How many accounts to skip from the start of the list.")]
    pub offset: u32,
}

/// X's max page size for the followers / following endpoints. We always
/// request full pages (rather than capping at the caller's `offset + count`)
/// so each page fetch is identical regardless of the requested slice — the
/// response cache can then serve it across calls.
const FOLLOW_LIST_PAGE: i32 = 1000;

/// X's max page size for `/2/tweets/search/recent` (used by `run_query` +
/// `get_replies`). Full pages for the same fewer-requests / cache reasons.
const SEARCH_PAGE: i32 = 100;

/// X's max page size for `/2/users/{id}/bookmarks`.
const BOOKMARKS_PAGE: i32 = 100;

/// One entry in a `list_following` / `list_followers` result.
#[derive(serde::Serialize)]
struct ListedUser {
    id: String,
    username: String,
    name: String,
}

fn project_user(u: User) -> ListedUser {
    ListedUser {
        id: u.id.0,
        username: u.username.0,
        name: u.name,
    }
}

#[tool_router(router = read_tools, vis = "pub")]
impl PsychologicalOperationsXApiMcp {
    #[tool(name = "get_replies", description = "Fetch recent replies to a tweet.")]
    async fn get_replies(
        &self,
        Parameters(req): Parameters<GetRepliesRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let target = req.tweet_id.clone();
                let need = req.offset as usize + req.count as usize;
                let base = tweets_search_recent::get::Request {
                    query: format!("conversation_id:{}", req.tweet_id),
                    start_time: None,
                    end_time: None,
                    since_id: None,
                    until_id: None,
                    max_results: Some(SEARCH_PAGE),
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
                let mut ids: Vec<String> = Vec::new();
                let mut next_token: Option<PaginationToken36> = None;
                loop {
                    let mut creq = base.clone();
                    creq.next_token = next_token.clone();
                    let resp = tweets_search_recent::http::get(&http, &auth, &creq).await?;
                    for t in resp.data.unwrap_or_default() {
                        let Some(id) = t.id.as_ref().map(|i| i.0.clone()) else {
                            continue;
                        };
                        let is_reply = t.referenced_tweets.as_ref().is_some_and(|refs| {
                            refs.iter().any(|r| {
                                matches!(r.type_, TweetReferencedTweetsItemType::RepliedTo)
                                    && r.id.0 == target
                            })
                        });
                        if is_reply {
                            ids.push(id);
                        }
                    }
                    if ids.len() >= need {
                        break;
                    }
                    match resp.meta.and_then(|m| m.next_token) {
                        Some(next) => next_token = Some(PaginationToken36(next.0)),
                        None => break,
                    }
                }
                let sliced: Vec<String> = ids
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .collect();
                let body = serde_json::to_string(&sliced)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "get_bio", description = "Fetch an X user's bio.")]
    async fn get_bio(
        &self,
        Parameters(req): Parameters<GetBioRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let creq = users_by_username::get::Request {
                    username: req.handle,
                    user_fields: Some(vec![params::UserFields::Description]),
                    expansions: None,
                    tweet_fields: None,
                };
                let resp = users_by_username::http::get(&http, &auth, &creq).await?;
                let body = resp.data.and_then(|u| u.description).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "get_profile_picture",
        description = "Fetch an X user's profile picture."
    )]
    async fn get_profile_picture(
        &self,
        Parameters(req): Parameters<GetProfilePictureRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let creq = users_by_username::get::Request {
                    username: req.handle,
                    user_fields: Some(vec![params::UserFields::ProfileImageUrl]),
                    expansions: None,
                    tweet_fields: None,
                };
                let resp = users_by_username::http::get(&http, &auth, &creq).await?;
                let body = resp
                    .data
                    .and_then(|u| u.profile_image_url.map(|url| url.to_string()))
                    .unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "get_tweet", description = "Fetch a tweet.")]
    async fn get_tweet(
        &self,
        Parameters(req): Parameters<GetTweetRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let creq = standard_tweet_request(&req.tweet_id);
                let resp = tweets_id::http::get(&http, &auth, &creq).await?;
                // No data block ⇒ the agent named a tweet that doesn't exist or
                // isn't visible to this account → agent-facing.
                let t = resp.data.ok_or_else(|| {
                    ToolError::agent(format!(
                        "tweet {} not found or not visible to this account",
                        req.tweet_id,
                    ))
                })?;
                let projected = project_tweet(&t, resp.includes.as_ref());
                let body = serde_json::to_string(&projected)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "open_attachment", description = "Fetch an attachment.")]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let creq = standard_tweet_request(&req.tweet_id);
                let resp = tweets_id::http::get(&http, &auth, &creq).await?;
                let (kind, mime) =
                    lookup_attachment(resp.includes.as_ref(), &req.url).ok_or_else(|| {
                        ToolError::agent(format!(
                            "attachment URL not on tweet {}: {}",
                            req.tweet_id, req.url,
                        ))
                    })?;
                let bytes = http.fetch_url(&req.url).await?;
                let fetched = FetchedAttachment { kind, mime, bytes };
                let b64 = base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);
                let body = match fetched.kind {
                    AttachmentKind::Photo => Content::image(b64, fetched.mime),
                    AttachmentKind::Video | AttachmentKind::AnimatedGif => {
                        Content::text(format!("data:{};base64,{}", fetched.mime, b64))
                    }
                };
                Ok(CallToolResult::success(vec![body]))
            }
            .await,
        )
    }

    #[tool(name = "run_query", description = "Run an X v2 recent search.")]
    async fn run_query(
        &self,
        Parameters(req): Parameters<RunQueryRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let need = req.offset as usize + req.count as usize;
                let base = standard_search_request(req.query);
                let mut projected: Vec<Tweet> = Vec::new();
                let mut next_token: Option<PaginationToken36> = None;
                loop {
                    let mut creq = base.clone();
                    creq.next_token = next_token.clone();
                    let resp = tweets_search_recent::http::get(&http, &auth, &creq).await?;
                    let includes = resp.includes;
                    for t in resp.data.unwrap_or_default().iter() {
                        projected.push(project_tweet(t, includes.as_ref()));
                    }
                    if projected.len() >= need {
                        break;
                    }
                    match resp.meta.and_then(|m| m.next_token) {
                        Some(next) => next_token = Some(PaginationToken36(next.0)),
                        None => break,
                    }
                }
                let sliced: Vec<Tweet> = projected
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .collect();
                let body = serde_json::to_string(&sliced)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "whoami",
        description = "Fetch your own X account's handle (@username)."
    )]
    async fn whoami(
        &self,
        Parameters(_req): Parameters<WhoamiRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let creq = users_me::get::Request {
                    user_fields: Some(vec![params::UserFields::Username]),
                    expansions: None,
                    tweet_fields: None,
                };
                let resp = users_me::http::get(&http, &auth, &creq).await?;
                let body = resp.data.map(|u| u.username.0).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "get_bookmarks", description = "Fetch your bookmarked tweets.")]
    async fn get_bookmarks(
        &self,
        Parameters(req): Parameters<GetBookmarksRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let user_id = resolve_self_user_id(&http, &auth).await?;
                let need = req.offset as usize + req.count as usize;
                let base = users_id_bookmarks::get::Request {
                    id: UserIdMatchesAuthenticatedUser(user_id),
                    max_results: Some(BOOKMARKS_PAGE),
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
                let mut projected: Vec<Tweet> = Vec::new();
                let mut pagination_token: Option<PaginationToken36> = None;
                loop {
                    let mut creq = base.clone();
                    creq.pagination_token = pagination_token.clone();
                    let resp = users_id_bookmarks::http::get(&http, &auth, &creq).await?;
                    let includes = resp.includes;
                    for t in resp.data.unwrap_or_default().iter() {
                        projected.push(project_tweet(t, includes.as_ref()));
                    }
                    if projected.len() >= need {
                        break;
                    }
                    match resp.meta.and_then(|m| m.next_token) {
                        Some(next) => pagination_token = Some(PaginationToken36(next.0)),
                        None => break,
                    }
                }
                let sliced: Vec<Tweet> = projected
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .collect();
                let body = serde_json::to_string(&sliced)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_following",
        description = "List the accounts an X user (by handle) follows."
    )]
    async fn list_following(
        &self,
        Parameters(req): Parameters<ListFollowingRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let target = resolve_handle_user_id(&http, &auth, req.handle).await?;
                let need = req.offset as usize + req.count as usize;
                let mut users: Vec<User> = Vec::new();
                let mut pagination_token: Option<PaginationToken32> = None;
                loop {
                    let creq = users_id_following::get::Request {
                        id: target.clone(),
                        max_results: Some(FOLLOW_LIST_PAGE),
                        pagination_token: pagination_token.clone(),
                        user_fields: None,
                        expansions: None,
                        tweet_fields: None,
                    };
                    let resp = users_id_following::http::get(&http, &auth, &creq).await?;
                    users.extend(resp.data.unwrap_or_default());
                    if users.len() >= need {
                        break;
                    }
                    match resp.meta.and_then(|m| m.next_token) {
                        Some(next) => pagination_token = Some(PaginationToken32(next.0)),
                        None => break,
                    }
                }
                let listed: Vec<ListedUser> = users
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .map(project_user)
                    .collect();
                let body = serde_json::to_string(&listed)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(
        name = "list_followers",
        description = "List the accounts that follow an X user (by handle)."
    )]
    async fn list_followers(
        &self,
        Parameters(req): Parameters<ListFollowersRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let target = resolve_handle_user_id(&http, &auth, req.handle).await?;
                let need = req.offset as usize + req.count as usize;
                let mut users: Vec<User> = Vec::new();
                let mut pagination_token: Option<PaginationToken32> = None;
                loop {
                    let creq = users_id_followers::get::Request {
                        id: target.clone(),
                        max_results: Some(FOLLOW_LIST_PAGE),
                        pagination_token: pagination_token.clone(),
                        user_fields: None,
                        expansions: None,
                        tweet_fields: None,
                    };
                    let resp = users_id_followers::http::get(&http, &auth, &creq).await?;
                    users.extend(resp.data.unwrap_or_default());
                    if users.len() >= need {
                        break;
                    }
                    match resp.meta.and_then(|m| m.next_token) {
                        Some(next) => pagination_token = Some(PaginationToken32(next.0)),
                        None => break,
                    }
                }
                let listed: Vec<ListedUser> = users
                    .into_iter()
                    .skip(req.offset as usize)
                    .take(req.count as usize)
                    .map(project_user)
                    .collect();
                let body = serde_json::to_string(&listed)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }
}
