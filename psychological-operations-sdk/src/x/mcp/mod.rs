//! X-API MCP-specific SDK surface. Today: per-(agent, tweet_id)
//! engagement record so the MCP write tools (`like`, `retweet`,
//! `reply`, `quote`) don't double-act on the same tweet, plus the
//! per-caller API request log that backs read/write quotas.

pub mod engagement;
pub mod request_log;

pub use engagement::{Engagement, EngagementStore};
pub use request_log::RequestLogStore;
