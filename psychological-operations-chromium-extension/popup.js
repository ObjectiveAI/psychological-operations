const identityEl = document.getElementById("identity");
const captureBtn = document.getElementById("capture");
const xAppForm = document.getElementById("x_app_form");
const xAppSaveBtn = document.getElementById("bf_save");
const statusEl = document.getElementById("status");

const X_APP_FIELDS = [
  ["client_id",      "bf_client_id"],
  ["client_secret",  "bf_client_secret"],
  ["api_key",        "bf_api_key"],
  ["api_key_secret", "bf_api_key_secret"],
  ["bearer_token",   "bf_bearer_token"],
];

let activeTabId = null;
let countTimer  = null;
let mode        = null; // "psyop" | "x_app"

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
  if (mode !== "psyop") return;
  const id = await activeTab();
  if (id == null) return;
  try {
    const reply = await chrome.tabs.sendMessage(id, { kind: "count" });
    const n = (reply && reply.count) || 0;
    captureBtn.textContent = `Capture (${n} tweet${n === 1 ? "" : "s"})`;
    captureBtn.disabled = n === 0;
  } catch (_) {
    captureBtn.textContent = "Capture (not an X page)";
    captureBtn.disabled = true;
  }
}

async function loadIdentity() {
  let reply;
  try {
    reply = await chrome.runtime.sendMessage({ kind: "popup_get_identity" });
  } catch (e) {
    identityEl.textContent = `identity error: ${e.message || e}`;
    identityEl.classList.add("error");
    return;
  }
  if (reply && reply.ok) {
    const id = reply.identity;
    identityEl.textContent = `psyop: ${id.psyop} @ ${id.commit.slice(0, 8)}`;
    identityEl.classList.remove("error");
    mode = "psyop";
    captureBtn.hidden = false;
    xAppForm.hidden = true;
  } else {
    // Identity unresolvable → x_app profile (PSYOP_NAME unset).
    identityEl.textContent = "x_app setup";
    identityEl.classList.remove("error");
    mode = "x_app";
    captureBtn.hidden = true;
    xAppForm.hidden = false;
  }
}

captureBtn.addEventListener("click", async () => {
  captureBtn.disabled = true;
  setStatus("extracting…");
  try {
    const id = await activeTab();
    const extractReply = await chrome.tabs.sendMessage(id, { kind: "extract" });
    const tweets = (extractReply && extractReply.tweets) || [];
    if (tweets.length === 0) {
      setStatus("nothing to capture", "error");
      captureBtn.disabled = false;
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
    captureBtn.disabled = false;
    refreshCount();
  }
});

xAppForm.addEventListener("submit", async (ev) => {
  ev.preventDefault();
  xAppSaveBtn.disabled = true;

  const credentials = {};
  let nonEmpty = 0;
  for (const [key, inputId] of X_APP_FIELDS) {
    const v = document.getElementById(inputId).value.trim();
    if (v.length > 0) {
      credentials[key] = v;
      nonEmpty++;
    } else {
      credentials[key] = null;
    }
  }
  if (nonEmpty === 0) {
    setStatus("enter at least one field", "error");
    xAppSaveBtn.disabled = false;
    return;
  }

  setStatus(`saving ${nonEmpty} field${nonEmpty === 1 ? "" : "s"}…`);
  try {
    const reply = await chrome.runtime.sendMessage({ kind: "popup_x_app_save", credentials });
    if (reply && reply.kind === "x_app_save_ok") {
      setStatus(`saved ${nonEmpty} field${nonEmpty === 1 ? "" : "s"} to x_app.json`, "ok");
      // Clear inputs after a successful save so secrets don't linger
      // visible in the popup if the operator re-opens it.
      for (const [_key, inputId] of X_APP_FIELDS) {
        document.getElementById(inputId).value = "";
      }
    } else {
      setStatus(`error: ${(reply && reply.error) || "?"}`, "error");
    }
  } catch (e) {
    setStatus(`error: ${e.message || e}`, "error");
  } finally {
    xAppSaveBtn.disabled = false;
  }
});

window.addEventListener("unload", () => {
  if (countTimer) clearInterval(countTimer);
});

(async () => {
  await loadIdentity();
  if (mode === "psyop") {
    refreshCount();
    countTimer = setInterval(refreshCount, 500);
  }
})();
