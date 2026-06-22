//! Runtime evaluation against live [`Tweet`] rows. The `Filter`
//! struct + publish-time `validate` live in
//! `psychological_operations_sdk::cli::psyops::filter`; this file
//! is purely the evaluator — it runs the operator's `python` boolean
//! expression (via the `python` command) against a tweet's metrics.

use psychological_operations_sdk::cli::psyops::filter::Filter;

use crate::error::Error;
use crate::tweet::Tweet;

/// Returns `Ok(true)` iff every static `min_*` / `max_*` gate
/// passes AND, when present, the `python` expression
/// evaluates to `True`. Returns `Ok(false)` if any static gate
/// rejects. Returns `Err` on Python eval / type errors.
///
/// Static gates run first (cheap) so a tweet that's already
/// rejected on engagement counts never pays the Python cost.
pub async fn evaluate(
    f: &Filter,
    t: &Tweet,
    ctx: &crate::context::Context,
) -> Result<bool, Error> {
    if !static_pass(f, t) {
        return Ok(false);
    }
    match &f.python {
        None => Ok(true),
        Some(src) => evaluate_python(src, t, ctx).await,
    }
}

fn static_pass(f: &Filter, t: &Tweet) -> bool {
    if let Some(v) = f.min_likes {
        if t.likes < v {
            return false;
        }
    }
    if let Some(v) = f.max_likes {
        if t.likes > v {
            return false;
        }
    }
    if let Some(v) = f.min_retweets {
        if t.retweets < v {
            return false;
        }
    }
    if let Some(v) = f.max_retweets {
        if t.retweets > v {
            return false;
        }
    }
    if let Some(v) = f.min_replies {
        if t.replies < v {
            return false;
        }
    }
    if let Some(v) = f.max_replies {
        if t.replies > v {
            return false;
        }
    }
    if let Some(v) = f.min_impressions {
        if t.impressions < v {
            return false;
        }
    }
    if let Some(v) = f.max_impressions {
        if t.impressions > v {
            return false;
        }
    }
    if let Some(v) = f.min_age {
        if t.age < v {
            return false;
        }
    }
    if let Some(v) = f.max_age {
        if t.age > v {
            return false;
        }
    }

    // Per-impression ratio gates are skipped entirely when
    // impressions == 0 — there's no meaningful rate without a
    // denominator, and we don't want to silently reject rows just
    // because the impression count hasn't been observed yet.
    if t.impressions > 0 {
        let denom = t.impressions as f64;
        let likes_pi = t.likes as f64 / denom;
        let retweets_pi = t.retweets as f64 / denom;
        let replies_pi = t.replies as f64 / denom;
        if let Some(v) = f.min_likes_per_impression {
            if likes_pi < v {
                return false;
            }
        }
        if let Some(v) = f.max_likes_per_impression {
            if likes_pi > v {
                return false;
            }
        }
        if let Some(v) = f.min_retweets_per_impression {
            if retweets_pi < v {
                return false;
            }
        }
        if let Some(v) = f.max_retweets_per_impression {
            if retweets_pi > v {
                return false;
            }
        }
        if let Some(v) = f.min_replies_per_impression {
            if replies_pi < v {
                return false;
            }
        }
        if let Some(v) = f.max_replies_per_impression {
            if replies_pi > v {
                return false;
            }
        }
    }
    true
}

async fn evaluate_python(
    src: &str,
    t: &Tweet,
    ctx: &crate::context::Context,
) -> Result<bool, Error> {
    let input = serde_json::json!({
        "likes": t.likes,
        "retweets": t.retweets,
        "replies": t.replies,
        "impressions": t.impressions,
        "age": t.age,
    });
    let result = crate::psyops::pyeval::run(ctx, src, input).await?;
    result
        .as_bool()
        .ok_or_else(|| Error::Other("filter python expression must evaluate to bool".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tweet::tw_default;

    fn tw(likes: u64, retweets: u64, replies: u64, impressions: u64, age: u64) -> Tweet {
        Tweet {
            likes,
            retweets,
            replies,
            impressions,
            age,
            ..tw_default("test")
        }
    }

    // The static engagement / ratio gates are sync and host-free, so they
    // are unit-tested here. The `python` path runs against the host's python
    // runtime and is exercised by the integration suite.

    #[test]
    fn static_min_max_gates() {
        let f = Filter {
            min_likes: Some(10),
            ..Default::default()
        };
        assert!(static_pass(&f, &tw(20, 0, 0, 0, 0)));
        assert!(!static_pass(&f, &tw(5, 0, 0, 0, 0)));
    }

    #[test]
    fn ratio_gates_apply() {
        // min 0.05 likes/impression — 5%+ engagement only
        let f = Filter {
            min_likes_per_impression: Some(0.05),
            ..Default::default()
        };
        assert!(static_pass(&f, &tw(60, 0, 0, 1000, 0))); // 6% — pass
        assert!(!static_pass(&f, &tw(40, 0, 0, 1000, 0))); // 4% — reject
        // zero impressions: ratio gates are skipped entirely, so this
        // passes despite the positive `min_likes_per_impression`.
        assert!(static_pass(&f, &tw(10, 0, 0, 0, 0)));
    }
}
