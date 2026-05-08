# psychological-operations-chrome

Sister-bundle directory: downloads upstream
[Chromium](https://www.chromium.org/) browser snapshots from
[`commondatastorage.googleapis.com/chromium-browser-snapshots/`](https://commondatastorage.googleapis.com/chromium-browser-snapshots/index.html?prefix=Win_x64/)
for the active Rust target triple and stages the extension so the
parent Rust crate can `include_bytes!` both into a single self-
contained binary.

We use upstream Chromium (BSD-licensed, freely redistributable, self-
identifies as "Chromium" in window titles + about page) rather than
Chrome for Testing (which is also Chromium-derived, but self-
identifies as "Google Chrome" / "Google Chrome for Testing").

Mirrors the build pipeline pattern used by
`objectiveai/objectiveai-claude-agent-sdk-runner/` — `build.sh`
produces / refreshes `embed/<target>/<profile>/`, `fingerprint.sh`
hashes the inputs, `validate.sh` is the contract for downstream
`build.rs` consumers.

## Files

- `VERSION` — pinned Chromium snapshot revisions, one per upstream
  platform (`WIN_X64`, `WIN`, `LINUX_X64`, `MAC`, `MAC_ARM`).
  Chromium snapshots run independent continuous builds per platform,
  so a single rev rarely exists everywhere; pin per-platform.
  Bumping is a per-line change followed by `bash build.sh` to refresh.
- `build.sh` — downloads the snapshot zip for the target, stages the
  extension, writes `launch-entry.txt` (relative path to chrome.exe /
  chrome / Chromium.app inside the zip) and `bundle.meta.json`. Logs go
  to `.logs/build/psychological-operations-chrome.txt`.
- `fingerprint.sh` — SHA256 of (VERSION + build.sh + every file in
  `psychological-operations-chrome-extension/`). Source it; sets
  `TARGET`, `PROFILE`, `CHROMIUM_REV`, `SNAPSHOT_PLATFORM`,
  `CHROMIUM_ZIP`, `CHROMIUM_LAUNCH_REL`, `CURRENT_FP`,
  `FINGERPRINT_FILE`.
- `validate.sh` — exits 0 if `embed/<target>/<profile>/` is fresh
  per the fingerprint, exits 1 if missing, 2 if stale. Called from
  `psychological-operations-cli/build.rs`.

## Output layout

```
embed/<rust-target-triple>/<debug|release>/
├── chrome-bundle.zip          ← Chromium snapshot zip, copied verbatim
├── extension.crx              ← signed extension (CRX3)
├── extension.tar              ← unpacked extension archive (for --load-extension)
├── launch-entry.txt           ← relative path to the Chromium binary inside the zip
├── bundle.meta.json           ← provenance (URL, rev, platform, byte count)
└── .fingerprint
```

The whole `embed/` tree is gitignored — these are large binary
artifacts produced from the pinned revs + extension sources.

## Usage

```sh
bash psychological-operations-chrome/build.sh                   # host target, debug
bash psychological-operations-chrome/build.sh --release         # host target, release
bash psychological-operations-chrome/build.sh --target x86_64-unknown-linux-gnu  # cross
```

Re-runs are no-ops via the fingerprint short-circuit unless the
extension files, the pinned Chromium revisions, or the build script
itself have changed.

## Target → Chromium snapshot platform

| Rust target                                              | Snapshot platform | Zip name          | Launch entry                                          |
| -------------------------------------------------------- | ----------------- | ----------------- | ----------------------------------------------------- |
| `x86_64-pc-windows-msvc` / `x86_64-pc-windows-gnu`       | `Win_x64`         | `chrome-win.zip`  | `chrome-win/chrome.exe`                               |
| `i686-pc-windows-msvc` / `i686-pc-windows-gnu`           | `Win`             | `chrome-win.zip`  | `chrome-win/chrome.exe`                               |
| `x86_64-unknown-linux-gnu` / `x86_64-unknown-linux-musl` | `Linux_x64`       | `chrome-linux.zip`| `chrome-linux/chrome`                                 |
| `aarch64-apple-darwin`                                   | `Mac_Arm`         | `chrome-mac.zip`  | `chrome-mac/Chromium.app/Contents/MacOS/Chromium`     |
| `x86_64-apple-darwin`                                    | `Mac`             | `chrome-mac.zip`  | `chrome-mac/Chromium.app/Contents/MacOS/Chromium`     |

## Bumping the pinned revisions

```sh
for plat in Win_x64 Win Linux_x64 Mac Mac_Arm; do
  echo "$plat: $(curl -sf https://commondatastorage.googleapis.com/chromium-browser-snapshots/$plat/LAST_CHANGE)"
done
```

Edit `VERSION` with the new revs, then `bash build.sh --release`
to verify the new artifacts download cleanly.
