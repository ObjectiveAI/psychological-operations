//! Verifies that the codegen'd `MediaUnion` tagged enum round-trips
//! against the X v2 wire shapes — photo, video, animated_gif — even
//! though each subtype `#[serde(flatten)]`s `Media` (whose `type_`
//! field is demoted to `Option<String>` precisely so the enum tag
//! can consume the wire `type` field without the inner Media struct
//! failing to find it).

use psychological_operations_sdk::x::types::MediaUnion;

#[test]
fn deserialize_photo() {
    let json = r#"{
        "type": "photo",
        "media_key": "3_111",
        "url": "https://pbs.twimg.com/media/abc.jpg",
        "height": 1024,
        "width": 2048
    }"#;
    let m: MediaUnion = serde_json::from_str(json).expect("photo deserialize");
    match m {
        MediaUnion::Photo(p) => {
            assert_eq!(
                p.url.as_ref().map(|u| u.as_str()),
                Some("https://pbs.twimg.com/media/abc.jpg"),
            );
        }
        other => panic!("expected Photo, got {other:?}"),
    }
}

#[test]
fn deserialize_video() {
    let json = r#"{
        "type": "video",
        "media_key": "7_222",
        "preview_image_url": "https://pbs.twimg.com/preview.jpg",
        "variants": [
            { "content_type": "video/mp4", "url": "https://video.twimg.com/lo.mp4", "bit_rate": 320000 },
            { "content_type": "video/mp4", "url": "https://video.twimg.com/hi.mp4", "bit_rate": 2176000 },
            { "content_type": "application/x-mpegURL", "url": "https://video.twimg.com/playlist.m3u8" }
        ]
    }"#;
    let m: MediaUnion = serde_json::from_str(json).expect("video deserialize");
    match m {
        MediaUnion::Video(v) => {
            let variants = v.variants.expect("variants present");
            assert_eq!(variants.len(), 3);
            assert_eq!(
                variants[1].content_type.as_deref(),
                Some("video/mp4"),
            );
        }
        other => panic!("expected Video, got {other:?}"),
    }
}

#[test]
fn deserialize_animated_gif() {
    let json = r#"{
        "type": "animated_gif",
        "media_key": "16_333",
        "preview_image_url": "https://pbs.twimg.com/gif-preview.jpg",
        "variants": [
            { "content_type": "video/mp4", "url": "https://video.twimg.com/gif.mp4", "bit_rate": 0 }
        ]
    }"#;
    let m: MediaUnion = serde_json::from_str(json).expect("animated_gif deserialize");
    match m {
        MediaUnion::AnimatedGif(a) => {
            let variants = a.variants.expect("variants present");
            assert_eq!(variants.len(), 1);
            assert_eq!(
                variants[0].url.as_ref().map(|u| u.as_str()),
                Some("https://video.twimg.com/gif.mp4"),
            );
        }
        other => panic!("expected AnimatedGif, got {other:?}"),
    }
}

#[test]
fn unknown_discriminator_errors() {
    let json = r#"{"type": "hologram", "media_key": "x"}"#;
    let r: Result<MediaUnion, _> = serde_json::from_str(json);
    assert!(r.is_err(), "expected error for unknown discriminator");
}
