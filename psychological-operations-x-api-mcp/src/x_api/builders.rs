//! Request builders for the X v2 endpoints we hit. They bake the
//! standard "tweet + media + author" expansion set so every
//! read-side tool sees the same shape, and the
//! `send_create_tweet` helper centralizes the
//! `tweets POST` plumbing the post / reply / quote tools share.

use psychological_operations_sdk::x::Error as XError;
use psychological_operations_sdk::x::client::{AuthMode, Client};
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::tweets as tweets_root;
use psychological_operations_sdk::x::tweets::id as tweets_id;
use psychological_operations_sdk::x::tweets::search::recent as tweets_search_recent;
use psychological_operations_sdk::x::types::{TweetCreateRequest, TweetId};
use psychological_operations_sdk::x::users::me as users_me;
use rmcp::ErrorData;

use super::tool_error::ToolError;

pub(super) fn standard_tweet_request(tweet_id: &str) -> tweets_id::get::Request {
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

pub(super) fn standard_search_request(query: String) -> tweets_search_recent::get::Request {
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

/// Empty-init `TweetCreateRequest` (all fields None / default) so
/// tool bodies can `..empty_tweet_create_request()` and only set the
/// fields they care about.
pub(super) fn empty_tweet_create_request() -> TweetCreateRequest {
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
///
/// Returns the TYPED [`psychological_operations_sdk::x::Error`] (not
/// `ToolError`) so the reply/quote tools can inspect the X 403 problem
/// and queue the attempt instead of failing; `post` just `?`-converts it
/// to `ToolError` via the existing `From` impl.
pub(super) async fn send_create_tweet(
    http: &Client,
    auth: &AuthMode,
    body: TweetCreateRequest,
) -> Result<String, XError> {
    let req = tweets_root::post::Request { body };
    let resp = tweets_root::http::post(http, auth, &req).await?;
    serde_json::to_string(&resp.data).map_err(|e| XError::Other(e.to_string()))
}

/// Resolve the authenticated user's numeric id via `/users/me`.
/// Used by the engagement tools (like / retweet / bookmark /
/// get_bookmarks) that need the acting user id in the URL path.
pub(super) async fn resolve_self_user_id(
    http: &Client,
    auth: &AuthMode,
) -> Result<String, ToolError> {
    let req = users_me::get::Request {
        user_fields: None,
        expansions: None,
        tweet_fields: None,
    };
    let resp = users_me::http::get(http, auth, &req).await?;
    // A 200 with no `data` block is an unexpected X response, not an agent
    // input fault → system.
    let user = resp.data.ok_or_else(|| {
        ToolError::System(ErrorData::internal_error(
            "users/me had no data".to_string(),
            None,
        ))
    })?;
    Ok(user.id.0)
}
