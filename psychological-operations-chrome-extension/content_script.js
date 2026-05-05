// Walk the X DOM and pull every currently-rendered tweet into a
// JSON-serializable shape. Selectors are best-effort against the X
// SPA — they're concentrated here so DOM churn is a single-file fix.

(function () {
  if (window.__psyopContentScriptLoaded) return;
  window.__psyopContentScriptLoaded = true;

  // Parse "12.3K" / "1.4M" / "42" → integer. Returns 0 if no digits found.
  function parseCount(s) {
    if (!s) return 0;
    const m = String(s).replace(/,/g, "").match(/(\d+(?:\.\d+)?)\s*([KMB])?/i);
    if (!m) return 0;
    const n = parseFloat(m[1]);
    const mult = { K: 1e3, M: 1e6, B: 1e9 }[(m[2] || "").toUpperCase()] || 1;
    return Math.round(n * mult);
  }

  // Buttons inside a tweet have aria-labels containing the count, e.g.
  // "12.3K Likes. Like". Find the button by its data-testid and parse.
  function readMetric(article, testid) {
    const el = article.querySelector(`[data-testid="${testid}"]`);
    if (!el) return 0;
    const label = el.getAttribute("aria-label") || el.textContent || "";
    return parseCount(label);
  }

  function extractOne(article) {
    // The permalink anchor inside the tweet — `/<handle>/status/<id>`.
    let id = null, handle = null;
    for (const a of article.querySelectorAll('a[href*="/status/"]')) {
      const m = a.getAttribute("href").match(/^\/([^/]+)\/status\/(\d+)/);
      if (m) { handle = m[1]; id = m[2]; break; }
    }
    if (!id) return null;

    // Text — concat all child runs of [data-testid="tweetText"].
    const textEl = article.querySelector('[data-testid="tweetText"]');
    const text = textEl ? textEl.innerText : "";

    // Created — the <time> element's `datetime` attr is ISO 8601.
    const timeEl = article.querySelector("time[datetime]");
    const created = timeEl ? timeEl.getAttribute("datetime") : "";

    // Engagement counts.
    const likes    = readMetric(article, "like");
    const retweets = readMetric(article, "retweet");
    const replies  = readMetric(article, "reply");

    // Media — images from twimg media URLs; videos from <video> elements.
    const images = Array.from(
      article.querySelectorAll('img[src*="twimg.com/media/"]')
    ).map((img) => ({ url: img.src }));

    const videos = Array.from(article.querySelectorAll("video")).map((v) => ({
      url: v.src || v.getAttribute("poster") || "",
    })).filter((v) => v.url);

    return {
      id, handle, text, created,
      likes, retweets, replies,
      images, videos,
    };
  }

  function extractTweets() {
    const out = [];
    const seen = new Set();
    for (const article of document.querySelectorAll('article[data-testid="tweet"]')) {
      const t = extractOne(article);
      if (!t) continue;
      if (seen.has(t.id)) continue;
      seen.add(t.id);
      out.push(t);
    }
    return out;
  }

  function countTweets() {
    return document.querySelectorAll('article[data-testid="tweet"]').length;
  }

  chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (!msg || typeof msg.kind !== "string") return false;
    if (msg.kind === "count") {
      sendResponse({ count: countTweets() });
      return false;
    }
    if (msg.kind === "extract") {
      sendResponse({ tweets: extractTweets() });
      return false;
    }
    return false;
  });
})();
