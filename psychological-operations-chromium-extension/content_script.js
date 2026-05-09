// Walk the X DOM and pull every currently-rendered tweet's id. The
// extension's only job is to announce "I saw this id in for-you";
// the native-host bridge enqueues each id into for_you_queue and
// the rust runtime later hydrates the rest (engagement counts,
// text, media) via the X v2 API. Selectors are best-effort against
// the X SPA — concentrated here so DOM churn is a single-file fix.

(function () {
  if (window.__psyopContentScriptLoaded) return;
  window.__psyopContentScriptLoaded = true;

  function extractOne(article) {
    // The permalink anchor inside the tweet — `/<handle>/status/<id>`.
    for (const a of article.querySelectorAll('a[href*="/status/"]')) {
      const m = a.getAttribute("href").match(/^\/[^/]+\/status\/(\d+)/);
      if (m) return { id: m[1] };
    }
    return null;
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
