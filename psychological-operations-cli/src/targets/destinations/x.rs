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
    use psychological_operations_x_api::x::http::Http;
    use psychological_operations_x_api::x::types::{
        TweetId, UserIdMatchesAuthenticatedUser,
        UsersLikesCreateRequest, UsersRetweetsCreateRequest,
    };

    let Subject::Psyop { name, psyop, output } = subject;

    let http = Http::for_psyop(
        reqwest::Client::new(),
        name,
        psyop.mock_enabled(),
        rt,
    ).await?;

    // Resolve the acting user via /2/users/me so the like/retweet
    // URLs can fill the {id} path segment.
    let me_req = psychological_operations_x_api::x::users::me::get::Request {
        user_fields: None, expansions: None, tweet_fields: None,
    };
    let me = psychological_operations_x_api::x::users::me::http::get(&http, &me_req).await
        .map_err(|e| crate::error::Error::Other(format!("/2/users/me failed: {e}")))?;
    let me_user = me.data.ok_or_else(|| crate::error::Error::Other(
        "/2/users/me returned no `data`".into(),
    ))?;
    let acting_id = UserIdMatchesAuthenticatedUser(me_user.id.0.clone());

    for scored in *output {
        let tweet_id = TweetId(scored.post.id.clone());
        match cfg.r#type {
            XType::Like => {
                let req = psychological_operations_x_api::x::users::id::likes::post::Request {
                    id: acting_id.clone(),
                    body: Some(UsersLikesCreateRequest { tweet_id }),
                };
                psychological_operations_x_api::x::users::id::likes::http::post(&http, &req).await
                    .map_err(|e| crate::error::Error::Other(format!(
                        "x like failed for tweet {}: {e}", scored.post.id,
                    )))?;
            }
            XType::Retweet => {
                let req = psychological_operations_x_api::x::users::id::retweets::post::Request {
                    id: acting_id.clone(),
                    body: Some(UsersRetweetsCreateRequest { tweet_id }),
                };
                psychological_operations_x_api::x::users::id::retweets::http::post(&http, &req).await
                    .map_err(|e| crate::error::Error::Other(format!(
                        "x retweet failed for tweet {}: {e}", scored.post.id,
                    )))?;
            }
        }
    }
    Ok(())
}
