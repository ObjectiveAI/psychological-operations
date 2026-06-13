pub use psychological_operations_sdk::cli::destinations::x::{X, XType};

use super::Subject;

pub async fn send(cfg: &X, subject: &Subject<'_>, ctx: &crate::context::Context) -> Result<(), crate::error::Error> {
    use psychological_operations_sdk::x::client::{AuthMode, Client};
    use psychological_operations_sdk::x::types::{
        TweetId, UserIdMatchesAuthenticatedUser,
        UsersLikesCreateRequest, UsersRetweetsCreateRequest,
    };

    let Subject::Psyop { name, psyop, output } = subject;

    let client = Client::new(
        reqwest::Client::new(),
        psyop.mock_enabled(),
        ctx.cache_max_size,
        ctx.cache_ttl,
        ctx.config.state_dir(),
        AuthMode::Psyop(name.to_string()),
        ctx.db.clone(),
    );

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
        let tweet_id = TweetId(scored.id.clone());
        match cfg.r#type {
            XType::Like => {
                let req = psychological_operations_sdk::x::users::id::likes::post::Request {
                    id: acting_id.clone(),
                    body: Some(UsersLikesCreateRequest { tweet_id }),
                };
                psychological_operations_sdk::x::users::id::likes::http::post(&client, &req).await
                    .map_err(|e| crate::error::Error::Other(format!(
                        "x like failed for tweet {}: {e}", scored.id,
                    )))?;
            }
            XType::Retweet => {
                let req = psychological_operations_sdk::x::users::id::retweets::post::Request {
                    id: acting_id.clone(),
                    body: Some(UsersRetweetsCreateRequest { tweet_id }),
                };
                psychological_operations_sdk::x::users::id::retweets::http::post(&client, &req).await
                    .map_err(|e| crate::error::Error::Other(format!(
                        "x retweet failed for tweet {}: {e}", scored.id,
                    )))?;
            }
        }
    }
    Ok(())
}
