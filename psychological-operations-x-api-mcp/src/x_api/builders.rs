//! Request builders for the X v2 endpoints we hit. They bake the
//! standard "tweet + media + author" expansion set so every
//! read-side tool sees the same shape, and the
//! `send_create_tweet` helper centralizes the
//! `tweets POST` plumbing the post / reply / quote tools share.

use psychological_operations_sdk::x::client::Client;
use psychological_operations_sdk::x::params;
use psychological_operations_sdk::x::tweets as tweets_root;
use psychological_operations_sdk::x::tweets::id as tweets_id;
use psychological_operations_sdk::x::tweets::search::recent as tweets_search_recent;
use psychological_operations_sdk::x::types::{TweetCreateRequest, TweetId};
use rmcp::ErrorData;

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
pub(super) async fn send_create_tweet(
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
