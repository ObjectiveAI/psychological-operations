//! Per-item scoring input builders.
//!
//! Each post/message becomes an objectiveai
//! [`InputValue`](objectiveai_sdk::functions::expression::InputValue) object,
//! constructed directly (no custom DTO + JSON round-trip) so media parts land
//! as the correct `RichContentPart` variants rather than plain objects.

use indexmap::IndexMap;
use objectiveai_sdk::agent::completions::message::{ImageUrl, RichContentPart, VideoUrl};
use objectiveai_sdk::functions::expression::InputValue;

use crate::db::{MediaUrl, Post};

/// Build the `images`/`videos` array as typed rich-content parts (empty when
/// the flag is off).
fn media_parts(urls: &[MediaUrl], include: bool, image: bool) -> InputValue {
    if !include {
        return InputValue::Array(Vec::new());
    }
    let parts = urls
        .iter()
        .map(|m| {
            let part = if image {
                RichContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: m.url.clone(),
                        detail: None,
                    },
                }
            } else {
                RichContentPart::VideoUrl {
                    video_url: VideoUrl { url: m.url.clone() },
                }
            };
            InputValue::RichContentPart(part)
        })
        .collect();
    InputValue::Array(parts)
}

fn text_value(text: &str, include: bool) -> InputValue {
    InputValue::String(if include { text.to_string() } else { String::new() })
}

/// X (tweet) scoring input: `{ tweet_id, text, images, videos }`.
pub fn new_post_input_value(
    post: &Post,
    include_text: bool,
    include_images: bool,
    include_videos: bool,
) -> InputValue {
    let mut map = IndexMap::new();
    map.insert("tweet_id".to_string(), InputValue::String(post.id.clone()));
    map.insert("text".to_string(), text_value(&post.text, include_text));
    map.insert(
        "images".to_string(),
        media_parts(&post.images, include_images, true),
    );
    map.insert(
        "videos".to_string(),
        media_parts(&post.videos, include_videos, false),
    );
    InputValue::Object(map)
}

/// Discord scoring input: `{ message_id, channel_id, text, images, videos }`.
/// `post.id` is the message id; `channel_id` is carried alongside (the `Post`
/// has no slot for it).
pub fn new_discord_input_value(
    post: &Post,
    channel_id: &str,
    include_text: bool,
    include_images: bool,
    include_videos: bool,
) -> InputValue {
    let mut map = IndexMap::new();
    map.insert("message_id".to_string(), InputValue::String(post.id.clone()));
    map.insert(
        "channel_id".to_string(),
        InputValue::String(channel_id.to_string()),
    );
    map.insert("text".to_string(), text_value(&post.text, include_text));
    map.insert(
        "images".to_string(),
        media_parts(&post.images, include_images, true),
    );
    map.insert(
        "videos".to_string(),
        media_parts(&post.videos, include_videos, false),
    );
    InputValue::Object(map)
}
