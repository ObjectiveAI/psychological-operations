use serde::Serialize;
use indexmap::IndexMap;
use objectiveai_sdk::functions::expression::{
    InputSchema, ObjectInputSchema, ObjectInputSchemaType,
    ArrayInputSchema, ArrayInputSchemaType,
    StringInputSchema, StringInputSchemaType,
    ImageInputSchema, ImageInputSchemaType,
    VideoInputSchema, VideoInputSchemaType,
};
use objectiveai_sdk::functions::alpha_vector::expression::VectorFunctionInputSchema;
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

/// Build the ObjectInputSchema for a single post (text + images + videos).
pub fn post_object_schema() -> ObjectInputSchema {
    let mut properties = IndexMap::new();

    properties.insert("text".to_string(), InputSchema::String(StringInputSchema {
        r#type: StringInputSchemaType::String,
        description: Some("The text content of the post.".to_string()),
        r#enum: None,
    }));

    properties.insert("images".to_string(), InputSchema::Array(ArrayInputSchema {
        r#type: ArrayInputSchemaType::Array,
        description: Some("Images attached to the post.".to_string()),
        items: Box::new(InputSchema::Image(ImageInputSchema {
            r#type: ImageInputSchemaType::Image,
            description: Some("An image URL.".to_string()),
        })),
        min_items: None,
        max_items: None,
    }));

    properties.insert("videos".to_string(), InputSchema::Array(ArrayInputSchema {
        r#type: ArrayInputSchemaType::Array,
        description: Some("Videos attached to the post.".to_string()),
        items: Box::new(InputSchema::Video(VideoInputSchema {
            r#type: VideoInputSchemaType::Video,
            description: Some("A video URL.".to_string()),
        })),
        min_items: None,
        max_items: None,
    }));

    ObjectInputSchema {
        r#type: ObjectInputSchemaType::Object,
        description: Some("A read post with text, images, and videos.".to_string()),
        properties,
        required: Some(vec!["text".to_string(), "images".to_string(), "videos".to_string()]),
    }
}

/// Build the scalar input schema (the post object directly).
pub fn scalar_input_schema() -> ObjectInputSchema {
    post_object_schema()
}

/// Build the vector input schema ({ items: [post, ...] }).
pub fn vector_input_schema() -> VectorFunctionInputSchema {
    VectorFunctionInputSchema {
        context: None,
        items: InputSchema::Object(post_object_schema()),
    }
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
