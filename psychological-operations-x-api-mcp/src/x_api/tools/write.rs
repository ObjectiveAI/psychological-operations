//! Write tools — mutations on X.
//!
//! Each tool acts as the session's `tag` (from the
//! `X-OBJECTIVEAI-ARGUMENTS` header) — the client is built bare and the
//! persona is `AuthMode::Agent(tag)`; mode-gating and the per-tag quota
//! gate run centrally in `call_tool` before dispatch.
//!
//! Each body runs inside [`finish`] so failures classify (see
//! [`super::super::tool_error`]): authorization-resolution and infra
//! errors surface as protocol errors; the authorized request's own
//! rejections (e.g. a 403 for replying to a replies-disabled tweet)
//! surface as `is_error` tool results the agent can act on.

use psychological_operations_db::{ReplyQuoteEntry, unix_now};
use psychological_operations_sdk::x::Error as XError;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::types::{
    BookmarkAddRequest, Problem, TweetCreateRequest, TweetCreateRequestReply, TweetId, TweetText,
    UserId, UserIdMatchesAuthenticatedUser, UsersFollowingCreateRequest, UsersLikesCreateRequest,
    UsersRetweetsCreateRequest,
};
use psychological_operations_sdk::x::users::by::username::username as users_by_username;
use psychological_operations_sdk::x::users::id::bookmarks as users_id_bookmarks;
use psychological_operations_sdk::x::users::id::following as users_id_following;
use psychological_operations_sdk::x::users::id::likes as users_id_likes;
use psychological_operations_sdk::x::users::id::retweets as users_id_retweets;
use psychological_operations_sdk::x::users::source_user_id::following::target_user_id as users_unfollow;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::super::PsychologicalOperationsXApiMcp;
use super::super::builders::{empty_tweet_create_request, resolve_self_user_id, send_create_tweet};
use super::super::tool_error::{ToolError, finish};

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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FollowRequest {
    #[schemars(description = "Handle (username, no leading @) of the user to follow.")]
    pub handle: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UnfollowRequest {
    #[schemars(description = "Handle (username, no leading @) of the user to unfollow.")]
    pub handle: String,
}

#[tool_router(router = write_tools, vis = "pub")]
impl PsychologicalOperationsXApiMcp {
    #[tool(name = "post", description = "Post a new tweet.")]
    async fn post(
        &self,
        Parameters(req): Parameters<PostRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                check_tweet_length(&req.text)?;
                let body = TweetCreateRequest {
                    text: Some(TweetText(req.text)),
                    ..empty_tweet_create_request()
                };
                let result = send_create_tweet(&http, &auth, body).await?;
                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
            .await,
        )
    }

    #[tool(name = "reply", description = "Reply to a tweet.")]
    async fn reply(
        &self,
        Parameters(req): Parameters<ReplyRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag.clone());

                // Capture the args before they move into the request, in
                // case we need to queue the attempt below.
                check_tweet_length(&req.text)?;
                let target_tweet_id = req.in_reply_to_tweet_id.clone();
                let text = req.text.clone();
                let body = TweetCreateRequest {
                    text: Some(TweetText(req.text)),
                    reply: Some(TweetCreateRequestReply {
                        in_reply_to_tweet_id: TweetId(req.in_reply_to_tweet_id),
                        auto_populate_reply_metadata: None,
                        exclude_reply_user_ids: None,
                    }),
                    ..empty_tweet_create_request()
                };
                match send_create_tweet(&http, &auth, body).await {
                    Ok(result) => Ok(CallToolResult::success(vec![Content::text(result)])),
                    // X refuses replies to threads this account can't engage.
                    // Queue the attempt instead of failing it.
                    Err(XError::Problem { code, ref problem })
                        if code.as_u16() == 403 && is_conversation_forbidden(problem) =>
                    {
                        self.db
                            .reply_quote_enqueue(&ReplyQuoteEntry {
                                agent_tag: tag,
                                kind: "reply".into(),
                                target_tweet_id,
                                text,
                                queued_at: unix_now(),
                            })
                            .await
                            .map_err(|e| {
                                ToolError::System(ErrorData::internal_error(e.to_string(), None))
                            })?;
                        Ok(CallToolResult::success(vec![Content::text(
                            "Your reply has been queued and will be delivered later.".to_string(),
                        )]))
                    }
                    Err(e) => Err(ToolError::from(e)),
                }
            }
            .await,
        )
    }

    #[tool(name = "quote", description = "Quote a tweet.")]
    async fn quote(
        &self,
        Parameters(req): Parameters<QuoteRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag.clone());

                // Capture the args before they move into the request, in
                // case we need to queue the attempt below.
                check_tweet_length(&req.text)?;
                let target_tweet_id = req.quote_tweet_id.clone();
                let text = req.text.clone();
                let body = TweetCreateRequest {
                    text: Some(TweetText(req.text)),
                    quote_tweet_id: Some(TweetId(req.quote_tweet_id)),
                    ..empty_tweet_create_request()
                };
                match send_create_tweet(&http, &auth, body).await {
                    Ok(result) => Ok(CallToolResult::success(vec![Content::text(result)])),
                    // X refuses quotes of threads this account can't engage.
                    // Queue the attempt instead of failing it.
                    Err(XError::Problem { code, ref problem })
                        if code.as_u16() == 403 && is_conversation_forbidden(problem) =>
                    {
                        self.db
                            .reply_quote_enqueue(&ReplyQuoteEntry {
                                agent_tag: tag,
                                kind: "quote".into(),
                                target_tweet_id,
                                text,
                                queued_at: unix_now(),
                            })
                            .await
                            .map_err(|e| {
                                ToolError::System(ErrorData::internal_error(e.to_string(), None))
                            })?;
                        Ok(CallToolResult::success(vec![Content::text(
                            "Your quote has been queued and will be delivered later.".to_string(),
                        )]))
                    }
                    Err(e) => Err(ToolError::from(e)),
                }
            }
            .await,
        )
    }

    #[tool(name = "like", description = "Like a tweet.")]
    async fn like(
        &self,
        Parameters(req): Parameters<LikeRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

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
            }
            .await,
        )
    }

    #[tool(name = "retweet", description = "Retweet a tweet.")]
    async fn retweet(
        &self,
        Parameters(req): Parameters<RetweetRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

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
            }
            .await,
        )
    }

    #[tool(name = "bookmark", description = "Bookmark a tweet.")]
    async fn bookmark(
        &self,
        Parameters(req): Parameters<BookmarkRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

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
            }
            .await,
        )
    }

    #[tool(name = "follow", description = "Follow an X user by handle.")]
    async fn follow(
        &self,
        Parameters(req): Parameters<FollowRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let target_user_id = resolve_handle_user_id(&http, &auth, req.handle).await?;
                let user_id = resolve_self_user_id(&http, &auth).await?;
                let creq = users_id_following::post::Request {
                    id: UserIdMatchesAuthenticatedUser(user_id),
                    body: Some(UsersFollowingCreateRequest { target_user_id }),
                };
                let resp = users_id_following::http::post(&http, &auth, &creq).await?;
                let body = serde_json::to_string(&resp.data)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }

    #[tool(name = "unfollow", description = "Unfollow an X user by handle.")]
    async fn unfollow(
        &self,
        Parameters(req): Parameters<UnfollowRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let tag = self.resolve_session(&extensions).await?.tag.clone();
        finish(
            async move {
                let http = self.build_client();
                let auth = AuthMode::Agent(tag);

                let target_user_id = resolve_handle_user_id(&http, &auth, req.handle).await?;
                let user_id = resolve_self_user_id(&http, &auth).await?;
                let creq = users_unfollow::delete::Request {
                    source_user_id: UserIdMatchesAuthenticatedUser(user_id),
                    target_user_id,
                };
                let resp = users_unfollow::http::delete(&http, &auth, &creq).await?;
                let body = serde_json::to_string(&resp.data)?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            .await,
        )
    }
}

/// Resolve a handle (username) to its X user id, acting as the session.
/// A handle X doesn't know surfaces as an agent-visible error result so the
/// agent can correct it.
async fn resolve_handle_user_id(
    http: &Client,
    auth: &AuthMode,
    handle: String,
) -> Result<UserId, ToolError> {
    let creq = users_by_username::get::Request {
        username: handle.clone(),
        user_fields: None,
        expansions: None,
        tweet_fields: None,
    };
    let resp = users_by_username::http::get(http, auth, &creq).await?;
    resp.data
        .map(|u| u.id)
        .ok_or_else(|| ToolError::agent(format!("no X user found for handle @{handle}")))
}

/// X's standard tweet character limit. Posts/replies/quotes over this are
/// rejected by X; we reject proactively with an agent-visible message so the
/// agent shortens its text instead of erroring (or silently queuing) later.
const TWEET_CHAR_LIMIT: usize = 280;

/// Reject body text that exceeds the tweet character limit. Counts Unicode
/// scalar values — accurate for the plain prose agents write (X's weighted
/// count only diverges for URLs / emoji / CJK). Surfaces as an agent-visible
/// error result so the model can shorten and retry.
fn check_tweet_length(text: &str) -> Result<(), ToolError> {
    let n = text.chars().count();
    if n > TWEET_CHAR_LIMIT {
        return Err(ToolError::agent(format!(
            "text is {n} characters, over the {TWEET_CHAR_LIMIT}-character limit — shorten it and try again."
        )));
    }
    Ok(())
}

/// True for the X 403 "conversation restriction" problem — the account
/// isn't allowed to reply to / quote that thread ("…not allowed because
/// you have not been mentioned or are not part of the conversation
/// thread…"). Scoped by the substring so unrelated 403s (auth,
/// suspension) still surface as errors rather than being silently queued.
fn is_conversation_forbidden(problem: &Problem) -> bool {
    problem
        .detail
        .as_deref()
        .is_some_and(|d| d.contains("not allowed because"))
}
