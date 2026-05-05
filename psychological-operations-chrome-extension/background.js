// Service worker. Owns the single chrome.runtime.connectNative port to
// the psychological-operations native host. Lazily opens it; reopens
// on disconnect. Relays init / ingest messages from the popup.

const HOST_NAME = "com.objectiveai.psychological_operations";

let port = null;
// Pending reply handlers, FIFO. Native messaging doesn't carry request
// IDs, so the protocol relies on one-in-flight-at-a-time discipline,
// which the popup honors (sequential init -> ingest).
const pending = [];

function openPort() {
  if (port) return port;
  port = chrome.runtime.connectNative(HOST_NAME);
  port.onMessage.addListener((msg) => {
    const next = pending.shift();
    if (next) next.resolve(msg);
  });
  port.onDisconnect.addListener(() => {
    const err = chrome.runtime.lastError;
    while (pending.length) {
      const next = pending.shift();
      next.reject(new Error(err ? err.message : "native host disconnected"));
    }
    port = null;
  });
  return port;
}

function send(msg) {
  return new Promise((resolve, reject) => {
    let p;
    try { p = openPort(); }
    catch (e) { reject(e); return; }
    pending.push({ resolve, reject });
    try { p.postMessage(msg); }
    catch (e) {
      // remove the just-pushed handler since postMessage failed
      pending.pop();
      reject(e);
    }
  });
}

async function getIdentity() {
  const cached = await chrome.storage.session.get(["identity"]);
  if (cached.identity) return cached.identity;
  const reply = await send({ kind: "init" });
  if (reply.kind === "init_ok") {
    await chrome.storage.session.set({ identity: reply });
    return reply;
  }
  throw new Error(reply.error || "init failed");
}

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (!msg || typeof msg.kind !== "string") return false;

  if (msg.kind === "popup_get_identity") {
    getIdentity()
      .then((id) => sendResponse({ ok: true, identity: id }))
      .catch((e) => sendResponse({ ok: false, error: String(e.message || e) }));
    return true;
  }

  if (msg.kind === "popup_ingest") {
    send({ kind: "ingest", tweets: msg.tweets })
      .then((reply) => sendResponse(reply))
      .catch((e) => sendResponse({ kind: "ingest_err", error: String(e.message || e) }));
    return true;
  }

  return false;
});
