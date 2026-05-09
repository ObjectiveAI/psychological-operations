// Service worker. Owns a single chrome.runtime.connectNative port to
// the psychological-operations native host. Lazy-opens it; reopens
// on disconnect. Only relays the x_app credential save.

console.log("[psyops-auth-sw] background.js loaded at", Date.now());

const HOST_NAME = "com.objectiveai.psychological_operations";

let port = null;
// Pending reply handlers, FIFO. Native messaging doesn't carry
// request IDs, so the protocol relies on one-in-flight-at-a-time
// discipline (the popup honors this — only one save at a time).
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

// Port-based listener (popup uses chrome.runtime.connect to avoid
// the MV3 sendMessage / inactive-SW wake-up race).
chrome.runtime.onConnect.addListener((port) => {
  console.log("[psyops-auth-sw] onConnect:", port.name);
  if (port.name !== "popup") return;
  port.onMessage.addListener((msg) => {
    console.log("[psyops-auth-sw] popup msg:", msg && msg.kind);
    if (!msg || typeof msg.kind !== "string") return;
    if (msg.kind === "popup_x_app_save") {
      send({ kind: "x_app_save", credentials: msg.credentials })
        .then((reply) => {
          console.log("[psyops-auth-sw] native reply:", reply && reply.kind);
          try { port.postMessage(reply); } catch (_) {}
        })
        .catch((e) => {
          console.log("[psyops-auth-sw] native err:", String(e.message || e));
          try {
            port.postMessage({ kind: "x_app_save_err", error: String(e.message || e) });
          } catch (_) {}
        });
    }
  });
});
