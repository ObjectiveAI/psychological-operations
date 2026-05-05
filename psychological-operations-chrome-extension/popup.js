const identityEl = document.getElementById("identity");
const button     = document.getElementById("capture");
const statusEl   = document.getElementById("status");

let activeTabId = null;
let countTimer  = null;

function setStatus(text, cls) {
  statusEl.textContent = text;
  statusEl.className = cls || "";
}

async function activeTab() {
  if (activeTabId != null) return activeTabId;
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  activeTabId = tabs[0] ? tabs[0].id : null;
  return activeTabId;
}

async function refreshCount() {
  const id = await activeTab();
  if (id == null) return;
  try {
    const reply = await chrome.tabs.sendMessage(id, { kind: "count" });
    const n = (reply && reply.count) || 0;
    button.textContent = `Capture (${n} tweet${n === 1 ? "" : "s"})`;
    button.disabled = n === 0;
  } catch (_) {
    // Content script not loaded on this tab — likely not an X page.
    button.textContent = "Capture (not an X page)";
    button.disabled = true;
  }
}

async function loadIdentity() {
  try {
    const reply = await chrome.runtime.sendMessage({ kind: "popup_get_identity" });
    if (reply && reply.ok) {
      const id = reply.identity;
      identityEl.textContent = `psyop: ${id.psyop} @ ${id.commit.slice(0, 8)}`;
      identityEl.classList.remove("error");
    } else {
      identityEl.textContent = `identity error: ${reply ? reply.error : "?"}`;
      identityEl.classList.add("error");
    }
  } catch (e) {
    identityEl.textContent = `identity error: ${e.message || e}`;
    identityEl.classList.add("error");
  }
}

button.addEventListener("click", async () => {
  button.disabled = true;
  setStatus("extracting…");
  try {
    const id = await activeTab();
    const extractReply = await chrome.tabs.sendMessage(id, { kind: "extract" });
    const tweets = (extractReply && extractReply.tweets) || [];
    if (tweets.length === 0) {
      setStatus("nothing to capture", "error");
      button.disabled = false;
      return;
    }
    setStatus(`sending ${tweets.length}…`);
    const reply = await chrome.runtime.sendMessage({ kind: "popup_ingest", tweets });
    if (reply.kind === "ingest_ok") {
      setStatus(`inserted ${reply.inserted}, skipped ${reply.skipped}`, "ok");
    } else {
      setStatus(`error: ${reply.error || "?"}`, "error");
    }
  } catch (e) {
    setStatus(`error: ${e.message || e}`, "error");
  } finally {
    button.disabled = false;
    refreshCount();
  }
});

window.addEventListener("unload", () => {
  if (countTimer) clearInterval(countTimer);
});

loadIdentity();
refreshCount();
countTimer = setInterval(refreshCount, 500);
