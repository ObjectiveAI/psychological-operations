//! Projection: codegen `x::types::Tweet` + `Expansions` → the
//! agent-facing [`Tweet`] in [`super::model`]. Also covers the
//! lookups `open_attachment` needs for resolving a URL back to its
//! `(kind, mime)`.

use psychological_operations_sdk::x::types::{
    self as x_types, MediaUnion, TweetReferencedTweetsItemType, Variant,
};

use super::model::{Attachment, AttachmentKind, Tweet, TweetSummary};

/// Slim projection for the multi-tweet list tools: only `id`, `handle`, and
/// `replied_to` (the reply target id). Everything else is left for `get_tweet`
/// — keeps list payloads tiny.
pub(super) fn project_tweet_summary(
    t: &x_types::Tweet,
    includes: Option<&x_types::Expansions>,
) -> TweetSummary {
    let id = t.id.as_ref().map(|i| i.0.clone()).unwrap_or_default();
    let handle = resolve_handle(t.author_id.as_ref(), includes);
    let (mut replied_to, mut quoted, mut retweeted) = (None, None, None);
    if let Some(refs) = t.referenced_tweets.as_ref() {
        for r in refs {
            match r.type_ {
                TweetReferencedTweetsItemType::RepliedTo => replied_to = Some(r.id.0.clone()),
                TweetReferencedTweetsItemType::Quoted => quoted = Some(r.id.0.clone()),
                TweetReferencedTweetsItemType::Retweeted => retweeted = Some(r.id.0.clone()),
            }
        }
    }
    TweetSummary {
        id,
        handle,
        replied_to,
        quoted,
        retweeted,
    }
}

pub(super) fn project_tweet(t: &x_types::Tweet, includes: Option<&x_types::Expansions>) -> Tweet {
    let id = t.id.as_ref().map(|i| i.0.clone()).unwrap_or_default();
    let content = t.text.as_ref().map(|tx| tx.0.clone()).unwrap_or_default();
    let handle = resolve_handle(t.author_id.as_ref(), includes);
    let attachments = collect_attachments(t, includes);

    let (mut replied_to, mut quoted, mut retweeted) = (None, None, None);
    if let Some(refs) = t.referenced_tweets.as_ref() {
        for r in refs {
            match r.type_ {
                TweetReferencedTweetsItemType::RepliedTo => replied_to = Some(r.id.0.clone()),
                TweetReferencedTweetsItemType::Quoted => quoted = Some(r.id.0.clone()),
                TweetReferencedTweetsItemType::Retweeted => retweeted = Some(r.id.0.clone()),
            }
        }
    }

    // `public_metrics` itself is Option (None when not requested by
    // tweet_fields). `reply_count` is spec-required when present;
    // default to 0 if the whole object is missing.
    let reply_count = t
        .public_metrics
        .as_ref()
        .map(|m| m.reply_count)
        .unwrap_or(0);

    Tweet {
        id,
        handle,
        content,
        attachments,
        replied_to,
        quoted,
        retweeted,
        reply_count,
    }
}

fn resolve_handle(
    author_id: Option<&x_types::UserId>,
    includes: Option<&x_types::Expansions>,
) -> String {
    let Some(aid) = author_id else {
        return String::new();
    };
    let Some(users) = includes.and_then(|i| i.users.as_ref()) else {
        return String::new();
    };
    users
        .iter()
        .find(|u| u.id.0 == aid.0)
        .map(|u| u.username.0.clone())
        .unwrap_or_default()
}

fn collect_attachments(
    t: &x_types::Tweet,
    includes: Option<&x_types::Expansions>,
) -> Vec<Attachment> {
    let Some(media_keys) = t.attachments.as_ref().and_then(|a| a.media_keys.as_ref()) else {
        return Vec::new();
    };
    let Some(media) = includes.and_then(|i| i.media.as_ref()) else {
        return Vec::new();
    };
    media_keys
        .iter()
        .filter_map(|mk| {
            media
                .iter()
                .find(|m| media_key_matches(m, mk))
                .and_then(attachment_from_media)
        })
        .collect()
}

fn media_key_matches(m: &MediaUnion, mk: &x_types::MediaKey) -> bool {
    let inner_key = match m {
        MediaUnion::Photo(p) => p.flatten_0.media_key.as_ref(),
        MediaUnion::Video(v) => v.flatten_0.media_key.as_ref(),
        MediaUnion::AnimatedGif(a) => a.flatten_0.media_key.as_ref(),
    };
    inner_key.map(|k| k.0 == mk.0).unwrap_or(false)
}

fn attachment_from_media(m: &MediaUnion) -> Option<Attachment> {
    match m {
        MediaUnion::Photo(p) => Some(Attachment {
            kind: AttachmentKind::Photo,
            url: p.url.as_ref()?.to_string(),
        }),
        MediaUnion::Video(v) => best_video_variant(v.variants.as_deref()).map(|url| Attachment {
            kind: AttachmentKind::Video,
            url,
        }),
        MediaUnion::AnimatedGif(a) => {
            best_video_variant(a.variants.as_deref()).map(|url| Attachment {
                kind: AttachmentKind::AnimatedGif,
                url,
            })
        }
    }
}

/// Pick a playable video variant for this media — preferring
/// the lowest-bit-rate `video/*` rendition X served (trades
/// quality for transfer size; `open_attachment`'s base64 payload
/// back to the agent is dominated by raw bytes). If none of the
/// video variants carry a `bit_rate`, falls back to the first
/// video variant with a URL. Returns `None` when no `video/*`
/// variant exists at all — `attachment_from_media` then returns
/// `None` and `collect_attachments`'s `filter_map` drops the
/// attachment from the agent-facing Tweet.
///
/// The `video/` filter excludes `application/x-mpegURL` HLS
/// playlists (X serves those alongside the real video bytes but
/// they're text manifests, not self-contained playable bytes).
fn best_video_variant(variants: Option<&[Variant]>) -> Option<String> {
    let variants = variants?;
    let is_video = |v: &&Variant| {
        v.content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with("video/"))
    };

    // 1. Lowest-bit-rate among video variants that have a bit_rate.
    let lowest = variants
        .iter()
        .filter(is_video)
        .filter_map(|v| {
            let url = v.url.as_ref()?.to_string();
            let bit_rate = v.bit_rate?;
            Some((bit_rate, url))
        })
        .min_by_key(|(br, _)| *br)
        .map(|(_, url)| url);
    if lowest.is_some() {
        return lowest;
    }

    // 2. Fallback — first video variant with a URL (no bit_rate).
    variants
        .iter()
        .filter(is_video)
        .find_map(|v| v.url.as_ref().map(|u| u.to_string()))
}

pub(super) fn lookup_attachment(
    includes: Option<&x_types::Expansions>,
    url: &str,
) -> Option<(AttachmentKind, String)> {
    let media = includes?.media.as_ref()?;
    for m in media {
        match m {
            MediaUnion::Photo(p) => {
                if p.url.as_ref().map(|u| u.as_str()) == Some(url) {
                    return Some((AttachmentKind::Photo, mime_for_photo_url(url).to_string()));
                }
            }
            MediaUnion::Video(v) => {
                if let Some(found) = match_variant_url(v.variants.as_deref(), url) {
                    return Some((AttachmentKind::Video, found));
                }
            }
            MediaUnion::AnimatedGif(a) => {
                if let Some(found) = match_variant_url(a.variants.as_deref(), url) {
                    return Some((AttachmentKind::AnimatedGif, found));
                }
            }
        }
    }
    None
}

fn match_variant_url(variants: Option<&[Variant]>, target: &str) -> Option<String> {
    variants?.iter().find_map(|v| {
        if v.url.as_ref().map(|u| u.as_str()) == Some(target) {
            Some(
                v.content_type
                    .clone()
                    .unwrap_or_else(|| "video/mp4".to_string()),
            )
        } else {
            None
        }
    })
}

fn mime_for_photo_url(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}
