# Build the psychological-operations-browser binary together with its
# CEF runtime, zip them, and stage them under
# embed/<target>/<profile>/ for the CLI's build.rs to include_bytes!.
#
# Usage:
#   pwsh scripts/build-bundle.ps1                     # debug, host target
#   pwsh scripts/build-bundle.ps1 -Release            # release, host target
#   pwsh scripts/build-bundle.ps1 -Target x86_64-pc-windows-msvc
#
# Produces:
#   embed/<target>/<profile>/browser-bundle.zip
#   embed/<target>/<profile>/browser-entry.txt   (= "psychological-operations-browser.exe")

[CmdletBinding()]
param(
    [switch]$Release,
    [string]$Target = "",
    [switch]$SkipBuild,
    [switch]$NoZip
)

$ErrorActionPreference = "Stop"

$ScriptRoot = $PSScriptRoot
$BrowserRoot = Split-Path -Parent $ScriptRoot
$WorkspaceRoot = Split-Path -Parent $BrowserRoot

$Profile = if ($Release) { "release" } else { "debug" }
if (-not $Target) {
    # rustc -vV emits a line "host: <triple>". Parse it.
    $rustcOut = & rustc -vV
    $hostLine = $rustcOut | Select-String -Pattern '^host:\s+(\S+)$'
    if (-not $hostLine) { throw "could not determine host target from rustc -vV" }
    $Target = $hostLine.Matches[0].Groups[1].Value
}

Write-Host "==> build-bundle: target=$Target profile=$Profile"

# 1. Build the browser binary (+ frontend via beforeBuildCommand).
#    No `--target` — letting cargo use its default (host) target-dir
#    layout means we share build artifacts with `cargo check / build`
#    runs in the rest of the workspace, instead of forcing a duplicate
#    `target/<triple>/...` tree that would re-download the entire
#    CEF SDK and double disk usage.
if (-not $SkipBuild) {
    Push-Location $WorkspaceRoot
    try {
        $cargoArgs = @("build", "-p", "psychological-operations-browser")
        if ($Release) { $cargoArgs += "--release" }
        Write-Host "==> cargo $($cargoArgs -join ' ')"
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}

# 2. Stage runtime files. cef-dll-sys's build pipeline copies the
#    full CEF runtime alongside the browser exe; try the
#    target-triple-scoped layout first (used when callers pass
#    `cargo build --target ...`), then fall back to the default
#    target-dir layout (no `--target` passed).
$TargetDirCandidates = @(
    (Join-Path $WorkspaceRoot "target" | Join-Path -ChildPath $Target | Join-Path -ChildPath $Profile),
    (Join-Path $WorkspaceRoot "target" | Join-Path -ChildPath $Profile)
)
$TargetDir = $null
foreach ($candidate in $TargetDirCandidates) {
    if (Test-Path (Join-Path $candidate "psychological-operations-browser.exe")) {
        $TargetDir = $candidate
        break
    }
}
if (-not $TargetDir) {
    throw "could not find psychological-operations-browser.exe in any of:`n  $($TargetDirCandidates -join "`n  ")"
}
Write-Host "==> staging from $TargetDir"

# -NoZip stages straight into embed/ (the caller zips it); zip mode keeps the
# per-target embed/<triple>/<profile>/staging/ layout.
if ($NoZip) {
    $EmbedDir = Join-Path $BrowserRoot "embed"
    $Staging = $EmbedDir
} else {
    $EmbedDir = Join-Path $BrowserRoot "embed" | Join-Path -ChildPath $Target | Join-Path -ChildPath $Profile
    $Staging = Join-Path $EmbedDir "staging"
}
if (Test-Path $Staging) { Remove-Item -Recurse -Force $Staging }
New-Item -ItemType Directory -Force -Path $Staging | Out-Null

# Files to copy: the browser exe + the lib it loads + every CEF
# runtime file the cef-dll-sys build dropped next to it. The list
# is exhaustive on purpose — anything missing makes libcef.dll
# refuse to initialize at runtime.
$RuntimeFiles = @(
    "psychological-operations-browser.exe",
    "psychological_operations_browser_lib.dll",
    "psychological_operations_browser_helper.exe",
    "bootstrap.exe",
    "bootstrapc.exe",
    "libcef.dll",
    "chrome_elf.dll",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
    "resources.pak",
    "icudtl.dat",
    "v8_context_snapshot.bin",
    "d3dcompiler_47.dll",
    "dxcompiler.dll",
    "dxil.dll",
    "libEGL.dll",
    "libGLESv2.dll",
    "vk_swiftshader.dll",
    "vk_swiftshader_icd.json",
    "vulkan-1.dll"
)

foreach ($f in $RuntimeFiles) {
    $src = Join-Path $TargetDir $f
    if (-not (Test-Path $src)) {
        # bootstrap/helper aren't always present; skip silently.
        if ($f -in @("psychological_operations_browser_helper.exe", "bootstrap.exe", "bootstrapc.exe")) { continue }
        throw "missing runtime file: $src"
    }
    Copy-Item -Path $src -Destination (Join-Path $Staging $f)
}

# CEF locales/ — required for resources.pak's text bindings.
$LocalesSrc = Join-Path $TargetDir "locales"
if (-not (Test-Path $LocalesSrc)) { throw "missing CEF locales dir: $LocalesSrc" }
Copy-Item -Recurse -Path $LocalesSrc -Destination (Join-Path $Staging "locales")

# 3. browser-entry.txt + zip the staging dir flat — unless -NoZip, in which
#    case the staging dir (embed/<triple>/<profile>/) IS the output and the
#    caller (build.sh) zips it.
Write-Host ("==> staged {0}" -f $Staging)
if (-not $NoZip) {
    $EntryFile = Join-Path $EmbedDir "browser-entry.txt"
    "psychological-operations-browser.exe" | Out-File -FilePath $EntryFile -Encoding ascii -NoNewline
    $BundleZip = Join-Path $EmbedDir "browser-bundle.zip"
    if (Test-Path $BundleZip) { Remove-Item -Force $BundleZip }
    Write-Host "==> compressing $BundleZip"
    Compress-Archive -Path (Join-Path $Staging "*") -DestinationPath $BundleZip -CompressionLevel Optimal
    $BundleBytes = (Get-Item $BundleZip).Length
    Write-Host ("==> wrote {0} ({1:N0} bytes)" -f $BundleZip, $BundleBytes)
    Write-Host ("==> wrote {0}" -f $EntryFile)
}
