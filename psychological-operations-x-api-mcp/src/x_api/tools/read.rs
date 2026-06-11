//! Read tools — pure-GET endpoints. `get_bookmarks` lives here too
//! even though its endpoint shares a URL prefix with the `bookmark`
//! write: the GET is a read.

use base64::Engine;
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::tweets::id as tweets_id;
use psychological_operations_sdk::x::tweets::search::recent as tweets_search_recent;
use psychological_operations_sdk::x::types::{
    TweetReferencedTweetsItemType, UserIdMatchesAuthenticatedUser,
};
use psychological_operations_sdk::x::users::by::username::username as users_by_username;
use psychological_operations_sdk::x::users::id::bookmarks as users_id_bookmarks;
use psychological_operations_sdk::x::users::me as users_me;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::builders::{resolve_self_user_id, standard_search_request, standard_tweet_request};
use super::super::model::{AttachmentKind, FetchedAttachment, Tweet};
use super::super::projection::{lookup_attachment, project_tweet};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetRepliesRequest {
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
pub struct GetBookmarksRequest {}

#[tool_router(router = read_tools, vis = "pub")]
impl PsychologicalOperationsXApiMcp {
    #[tool(
        name = "get_replies",
        description = "Fetch recent replies to a tweet."
    )]
    async fn get_replies(
        &self,
        Parameters(req): Parameters<GetRepliesRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

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
        let resp = tweets_search_recent::http::get(&http, &creq)
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
        let body = serde_json::to_string(&ids)
            .map_err(|e| ErrorData::internal_error(format!("serialize ids: {e}"), None))?;
        self.respond_with_quota(&http, &state, Content::text(body)).await
    }

    #[tool(
        name = "get_bio",
        description = "Fetch an X user's bio."
    )]
    async fn get_bio(
        &self,
        Parameters(req): Parameters<GetBioRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let creq = users_by_username::get::Request {
            username: req.handle,
            user_fields: Some(vec![params::UserFields::Description]),
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_by_username::http::get(&http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/by/username: {e}"), None))?;
        let body = resp.data.and_then(|u| u.description).unwrap_or_default();
        self.respond_with_quota(&http, &state, Content::text(body)).await
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
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let creq = users_by_username::get::Request {
            username: req.handle,
            user_fields: Some(vec![params::UserFields::ProfileImageUrl]),
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_by_username::http::get(&http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/by/username: {e}"), None))?;
        let body = resp
            .data
            .and_then(|u| u.profile_image_url.map(|url| url.to_string()))
            .unwrap_or_default();
        self.respond_with_quota(&http, &state, Content::text(body)).await
    }

    #[tool(
        name = "get_tweet",
        description = "Fetch a tweet."
    )]
    async fn get_tweet(
        &self,
        Parameters(req): Parameters<GetTweetRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let creq = standard_tweet_request(&req.tweet_id);
        let resp = tweets_id::http::get(&http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("tweets/{{id}}: {e}"), None))?;
        let t = resp.data.ok_or_else(|| {
            ErrorData::internal_error(
                format!("tweet {} response had no data", req.tweet_id),
                None,
            )
        })?;
        let projected = project_tweet(&t, resp.includes.as_ref());
        let body = serde_json::to_string(&projected)
            .map_err(|e| ErrorData::internal_error(format!("serialize tweet: {e}"), None))?;
        self.respond_with_quota(&http, &state, Content::text(body)).await
    }

    #[tool(
        name = "open_attachment",
        description = "Fetch an attachment."
    )]
    async fn open_attachment(
        &self,
        Parameters(req): Parameters<OpenAttachmentRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let creq = standard_tweet_request(&req.tweet_id);
        let resp = tweets_id::http::get(&http, &creq)
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
        let bytes = http
            .fetch_url(&req.url)
            .await
            .map_err(|e| ErrorData::internal_error(format!("fetch_url: {e}"), None))?;
        let fetched = FetchedAttachment { kind, mime, bytes };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&fetched.bytes);
        let body = match fetched.kind {
            AttachmentKind::Photo => Content::image(b64, fetched.mime),
            AttachmentKind::Video | AttachmentKind::AnimatedGif => {
                Content::text(format!("data:{};base64,{}", fetched.mime, b64))
            }
        };
        self.respond_with_quota(&http, &state, body).await
    }

    #[tool(
        name = "run_query",
        description = "Run an X v2 recent search."
    )]
    async fn run_query(
        &self,
        Parameters(req): Parameters<RunQueryRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let creq = standard_search_request(req.query);
        let resp = tweets_search_recent::http::get(&http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("search: {e}"), None))?;
        let includes = resp.includes.as_ref();
        let projected: Vec<Tweet> = resp
            .data
            .unwrap_or_default()
            .iter()
            .map(|t| project_tweet(t, includes))
            .collect();
        let body = serde_json::to_string(&projected)
            .map_err(|e| ErrorData::internal_error(format!("serialize tweets: {e}"), None))?;
        self.respond_with_quota(&http, &state, Content::text(body)).await
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
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let req = users_me::get::Request {
            user_fields: Some(vec![params::UserFields::Username]),
            expansions: None,
            tweet_fields: None,
        };
        let resp = users_me::http::get(&http, &req)
            .await
            .map_err(|e| ErrorData::internal_error(format!("users/me: {e}"), None))?;
        let body = resp.data.map(|u| u.username.0).unwrap_or_default();
        self.respond_with_quota(&http, &state, Content::text(body)).await
    }

    #[tool(
        name = "get_bookmarks",
        description = "Fetch your bookmarked tweets."
    )]
    async fn get_bookmarks(
        &self,
        Parameters(_req): Parameters<GetBookmarksRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let http = self.build_client(&state);

        let user_id = resolve_self_user_id(&http).await?;
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
        let resp = users_id_bookmarks::http::get(&http, &creq)
            .await
            .map_err(|e| ErrorData::internal_error(format!("bookmarks: {e}"), None))?;
        let includes = resp.includes.as_ref();
        let projected: Vec<Tweet> = resp
            .data
            .unwrap_or_default()
            .iter()
            .map(|t| project_tweet(t, includes))
            .collect();
        let body = serde_json::to_string(&projected)
            .map_err(|e| ErrorData::internal_error(format!("serialize tweets: {e}"), None))?;
        self.respond_with_quota(&http, &state, Content::text(body)).await
    }
}
