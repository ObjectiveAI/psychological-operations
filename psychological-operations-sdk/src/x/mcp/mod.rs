//! X-API MCP-specific SDK surface. Today: per-(agent, tweet_id)
//! engagement record so the MCP write tools (`like`, `retweet`,
//! `reply`, `quote`) don't double-act on the same tweet.

pub mod engagement;

pub use engagement::{Engagement, EngagementStore};
