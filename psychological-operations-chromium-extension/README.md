# psychological-operations-chromium-extension

Chromium MV3 extension that captures the tweets currently rendered on
an X feed and writes them into the local `posts` table with
`for_you = true`. The extension is a single build; per-psyop state
is the **Chromium profile directory** plus `PSYOP_NAME` /
`PSYOP_COMMIT_SHA` env vars set when Chromium is launched.

## Components

- `manifest.json` — MV3 manifest, minimal permissions.
- `popup.{html,js,css}` — single button + live count of currently-DOM-
  rendered tweets + `psyop @ commit` status line.
- `background.js` — service worker; owns the
  `chrome.runtime.connectNative` port to the native host.
- `content_script.js` — DOM walker / tweet extractor. Selectors are
  centralized here so X DOM changes are a single-file fix.

## Wire protocol (extension ↔ native host)

Framed JSON over stdin/stdout per Chromium's native-messaging spec
(4-byte little-endian length, then UTF-8 JSON).

```
ext → host: {"kind":"init"}
host → ext: {"kind":"init_ok","psyop":"foo","commit":"abc12345…"}
                     | {"kind":"init_err","error":"…"}

ext → host: {"kind":"ingest","tweets":[{...}]}
host → ext: {"kind":"ingest_ok","inserted":24,"skipped":0}
                     | {"kind":"ingest_err","error":"…"}
```

Each tweet (extension → host):
```jsonc
{
  "id": "1234567890",
  "handle": "alice",
  "text": "...",
  "created": "2026-05-05T18:00:00Z",   // ISO 8601 from <time datetime>
  "likes": 42, "retweets": 12, "replies": 5,
  "images": [{"url": "https://pbs.twimg.com/media/..."}],
  "videos": [{"url": "https://..."}]
}
```

## Manual install (dev)

This is the manual flow until the embedded-Chromium runner exists
(it'll do all of this automatically per psyop).

### 1. Build the binary

```sh
cargo build -p psychological-operations-cli
```

### 2. Drop a wrapper script

Chromium's native-messaging manifest invokes a binary directly with
no args, so we need a tiny wrapper that calls
`psychological-operations native-host`:

Linux / macOS — save as `~/bin/psychological-operations-native-host.sh`,
`chmod +x`:
```sh
#!/bin/sh
exec /absolute/path/to/target/debug/psychological-operations native-host "$@"
```

Windows — save as `%USERPROFILE%\bin\psychological-operations-native-host.cmd`:
```cmd
@echo off
"C:\absolute\path\to\target\debug\psychological-operations.exe" native-host %*
```

### 3. Load the extension and copy its ID

In Chromium → `chrome://extensions` → Developer mode on →
"Load unpacked" → select `psychological-operations-chromium-extension/`. Copy the generated
extension ID (looks like `abcd1234efgh…`).

### 4. Drop the native-messaging manifest

Replace `<EXT_ID>` with the ID from step 3 and `<WRAPPER_PATH>`
with the absolute wrapper path from step 2.

**Linux**:
`~/.config/chromium/NativeMessagingHosts/com.objectiveai.psychological_operations.json`

**macOS**:
`~/Library/Application Support/Chromium/NativeMessagingHosts/com.objectiveai.psychological_operations.json`

**Windows**: register under `HKCU\Software\Chromium\NativeMessagingHosts\com.objectiveai.psychological_operations` (and `HKCU\Software\Google\Chrome\NativeMessagingHosts\...` for a Google Chrome side-load) with the JSON path as the default value. (See the [Chromium docs](https://www.chromium.org/developers/design-documents/native-messaging/) for the registry layout.)

```json
{
  "name": "com.objectiveai.psychological_operations",
  "description": "Psychological Operations native host",
  "path": "<WRAPPER_PATH>",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://<EXT_ID>/"]
}
```

### 5. Launch Chromium with the right env vars and a dedicated profile

```sh
PSYOP_NAME=test \
chromium --user-data-dir="$HOME/.psychological-operations/chromium-profiles/test"
```

`PSYOP_COMMIT_SHA` is optional — if unset, the host does
`git rev-parse HEAD` inside `<psyops_dir>/<PSYOP_NAME>/`.

(The profile must have the extension loaded too — easiest route is
to open `chrome://extensions`, enable developer mode, and load
unpacked once per profile.)

### 6. Capture

Visit `https://x.com/home`, click the extension toolbar icon, click
**Capture**. Verify:

```sh
sqlite3 ~/.psychological-operations/data.db \
  "SELECT id, handle, psyop, for_you, query
   FROM posts WHERE psyop='test'"
```

Rows should have `for_you = 1` and `query IS NULL`.

## Future (not in this commit)

A Rust subcommand will create a per-psyop Chromium profile, write the
native-messaging manifest with the right `allowed_origins`, pre-load
the extension into that profile, and `exec` Chromium with the env vars
set — replacing this whole manual checklist with a single command.
