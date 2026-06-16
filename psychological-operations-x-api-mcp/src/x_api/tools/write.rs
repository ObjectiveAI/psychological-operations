//! Write tools — mutations on X.
//!
//! Each tool acts as the session's `account` (from the
//! `X-OBJECTIVEAI-ARGUMENTS` header) — the client is built bare and the
//! persona is `AuthMode::Agent(account)`; mode-gating and the per-account
//! quota gate run centrally in `call_tool` before dispatch.
//!
//! Each body runs inside [`finish`] so failures classify (see
//! [`super::super::tool_error`]): authorization-resolution and infra
//! errors surface as protocol errors; the authorized request's own
//! rejections (e.g. a 403 for replying to a replies-disabled tweet)
//! surface as `is_error` tool results the agent can act on.

use psychological_operations_sdk::x::client::AuthMode;
use psychological_operations_sdk::x::types::{
    BookmarkAddRequest, TweetCreateRequest, TweetCreateRequestReply, TweetId, TweetText,
    UserIdMatchesAuthenticatedUser, UsersLikesCreateRequest, UsersRetweetsCreateRequest,
};
use psychological_operations_sdk::x::users::id::bookmarks as users_id_bookmarks;
use psychological_operations_sdk::x::users::id::likes as users_id_likes;
use psychological_operations_sdk::x::users::id::retweets as users_id_retweets;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::builders::{empty_tweet_create_request, resolve_self_user_id, send_create_tweet};
use super::super::tool_error::finish;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PostRequest {
    #[schemars(description = "Body text of the new tweet.")]
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReplyRequest {
    #[schemars(description = "Body text of the reply.")]
    pub text: String,
    #[schemars(description = "Numeric ID of the tweet being replied to.")]
    pub in_reply_to_tweet_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QuoteRequest {
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

#[tool_router(router = write_tools, vis = "pub")]
impl PsychologicalOperationsXApiMcp {
    #[tool(
        name = "post",
        description = "Post a new tweet."
    )]
    async fn post(
        &self,
        Parameters(req): Parameters<PostRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let http = self.build_client();
            let auth = AuthMode::Agent(account);

            let body = TweetCreateRequest {
                text: Some(TweetText(req.text)),
                ..empty_tweet_create_request()
            };
            let result = send_create_tweet(&http, &auth, body).await?;
            Ok(CallToolResult::success(vec![Content::text(result)]))
        }.await)
    }

    #[tool(
        name = "reply",
        description = "Reply to a tweet."
    )]
    async fn reply(
        &self,
        Parameters(req): Parameters<ReplyRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let http = self.build_client();
            let auth = AuthMode::Agent(account);

            let body = TweetCreateRequest {
                text: Some(TweetText(req.text)),
                reply: Some(TweetCreateRequestReply {
                    in_reply_to_tweet_id: TweetId(req.in_reply_to_tweet_id),
                    auto_populate_reply_metadata: None,
                    exclude_reply_user_ids: None,
                }),
                ..empty_tweet_create_request()
            };
            let result = send_create_tweet(&http, &auth, body).await?;
            Ok(CallToolResult::success(vec![Content::text(result)]))
        }.await)
    }

    #[tool(
        name = "quote",
        description = "Quote a tweet."
    )]
    async fn quote(
        &self,
        Parameters(req): Parameters<QuoteRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let http = self.build_client();
            let auth = AuthMode::Agent(account);

            let body = TweetCreateRequest {
                text: Some(TweetText(req.text)),
                quote_tweet_id: Some(TweetId(req.quote_tweet_id)),
                ..empty_tweet_create_request()
            };
            let result = send_create_tweet(&http, &auth, body).await?;
            Ok(CallToolResult::success(vec![Content::text(result)]))
        }.await)
    }

    #[tool(
        name = "like",
        description = "Like a tweet."
    )]
    async fn like(
        &self,
        Parameters(req): Parameters<LikeRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let http = self.build_client();
            let auth = AuthMode::Agent(account);

            let user_id = resolve_self_user_id(&http, &auth).await?;
            let creq = users_id_likes::post::Request {
                id: UserIdMatchesAuthenticatedUser(user_id),
                body: Some(UsersLikesCreateRequest {
                    tweet_id: TweetId(req.tweet_id),
                }),
            };
            let resp = users_id_likes::http::post(&http, &auth, &creq).await?;
            let body = serde_json::to_string(&resp.data)?;
            Ok(CallToolResult::success(vec![Content::text(body)]))
        }.await)
    }

    #[tool(
        name = "retweet",
        description = "Retweet a tweet."
    )]
    async fn retweet(
        &self,
        Parameters(req): Parameters<RetweetRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let http = self.build_client();
            let auth = AuthMode::Agent(account);

            let user_id = resolve_self_user_id(&http, &auth).await?;
            let creq = users_id_retweets::post::Request {
                id: UserIdMatchesAuthenticatedUser(user_id),
                body: Some(UsersRetweetsCreateRequest {
                    tweet_id: TweetId(req.tweet_id),
                }),
            };
            let resp = users_id_retweets::http::post(&http, &auth, &creq).await?;
            let body = serde_json::to_string(&resp.data)?;
            Ok(CallToolResult::success(vec![Content::text(body)]))
        }.await)
    }

    #[tool(
        name = "bookmark",
        description = "Bookmark a tweet."
    )]
    async fn bookmark(
        &self,
        Parameters(req): Parameters<BookmarkRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let account = self.resolve_session(&extensions).await?.account.clone();
        finish(async move {
            let http = self.build_client();
            let auth = AuthMode::Agent(account);

            let user_id = resolve_self_user_id(&http, &auth).await?;
            let creq = users_id_bookmarks::post::Request {
                id: UserIdMatchesAuthenticatedUser(user_id),
                body: BookmarkAddRequest {
                    tweet_id: TweetId(req.tweet_id),
                },
            };
            let resp = users_id_bookmarks::http::post(&http, &auth, &creq).await?;
            let body = serde_json::to_string(&resp.data)?;
            Ok(CallToolResult::success(vec![Content::text(body)]))
        }.await)
    }
}
