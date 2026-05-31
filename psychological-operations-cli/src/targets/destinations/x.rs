use serde::{Deserialize, Serialize};

use super::Subject;

/// "X" target — like or retweet each scored post on behalf of the
/// psyop's X account. The acting user is determined per-psyop via
/// the OAuth tokens at `~/.psychological-operations/tokens/<name>.json`,
/// silently refreshed if expired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X {
    /// Internal field name uses raw-keyword `r#type` to mirror the
    /// user's spec; on the wire it serializes as `"action"` to avoid
    /// collision with the parent `Destination`'s `"type"` tag.
    #[serde(rename = "action")]
    pub r#type: XType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XType {
    Like,
    Retweet,
}

pub async fn send(cfg: &X, subject: &Subject<'_>, rt: &crate::run::Config) -> Result<(), crate::error::Error> {
    use psychological_operations_sdk::x::client::Client;
    use psychological_operations_sdk::x::types::{
        TweetId, UserIdMatchesAuthenticatedUser,
        UsersLikesCreateRequest, UsersRetweetsCreateRequest,
    };

    let Subject::Psyop { name, psyop, output } = subject;

    let client = Client::for_psyop(
        reqwest::Client::new(),
        name,
        psyop.mock_enabled(),
        &rt.objectiveai_base_dir(),
        // Bytes — explicit per-call size budget for the SQLite response
        // cache. No `DEFAULT_*` constant — `Client::*` makes this a
        // required arg and every CLI callsite picks its own value.
        256 * 1024 * 1024,
        // Cache entry TTL — plumbed but unused today (future
        // time-based eviction will consume it).
        std::time::Duration::from_secs(3600),
    )
    .await
    .map_err(|e| crate::error::Error::Other(format!("Client::for_psyop: {e}")))?;

    // Resolve the acting user via /2/users/me so the like/retweet
    // URLs can fill the {id} path segment.
    let me_req = psychological_operations_sdk::x::users::me::get::Request {
        user_fields: None, expansions: None, tweet_fields: None,
    };
    let me = psychological_operations_sdk::x::users::me::http::get(&client, &me_req).await
        .map_err(|e| crate::error::Error::Other(format!("/2/users/me failed: {e}")))?;
    let me_user = me.data.ok_or_else(|| crate::error::Error::Other(
        "/2/users/me returned no `data`".into(),
    ))?;
    let acting_id = UserIdMatchesAuthenticatedUser(me_user.id.0.clone());

    for scored in *output {
        let tweet_id = TweetId(scored.post.id.clone());
        match cfg.r#type {
            XType::Like => {
                let req = psychological_operations_sdk::x::users::id::likes::post::Request {
                    id: acting_id.clone(),
                    body: Some(UsersLikesCreateRequest { tweet_id }),
                };
                psychological_operations_sdk::x::users::id::likes::http::post(&client, &req).await
                    .map_err(|e| crate::error::Error::Other(format!(
                        "x like failed for tweet {}: {e}", scored.post.id,
                    )))?;
            }
            XType::Retweet => {
                let req = psychological_operations_sdk::x::users::id::retweets::post::Request {
                    id: acting_id.clone(),
                    body: Some(UsersRetweetsCreateRequest { tweet_id }),
                };
                psychological_operations_sdk::x::users::id::retweets::http::post(&client, &req).await
                    .map_err(|e| crate::error::Error::Other(format!(
                        "x retweet failed for tweet {}: {e}", scored.post.id,
                    )))?;
            }
        }
    }
    Ok(())
}
