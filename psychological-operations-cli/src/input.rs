use serde::Serialize;
use crate::db::{Post, MediaUrl};

#[derive(Debug, Serialize)]
pub struct PostInputValue {
    pub text: String,
    pub images: Vec<ImagePart>,
    pub videos: Vec<VideoPart>,
}

#[derive(Debug, Serialize)]
pub struct ImagePart {
    pub r#type: &'static str,
    pub image_url: MediaUrl,
}

#[derive(Debug, Serialize)]
pub struct VideoPart {
    pub r#type: &'static str,
    pub video_url: MediaUrl,
}

#[derive(Debug, Serialize)]
pub struct PostsInputValue {
    pub items: Vec<PostInputValue>,
}

pub fn new_post_input_value(
    post: &Post,
    include_images: bool,
    include_videos: bool,
) -> PostInputValue {
    PostInputValue {
        text: post.text.clone(),
        images: if include_images {
            post.images.iter().map(|img| ImagePart {
                r#type: "image_url",
                image_url: MediaUrl { url: img.url.clone() },
            }).collect()
        } else {
            Vec::new()
        },
        videos: if include_videos {
            post.videos.iter().map(|vid| VideoPart {
                r#type: "video_url",
                video_url: MediaUrl { url: vid.url.clone() },
            }).collect()
        } else {
            Vec::new()
        },
    }
}
