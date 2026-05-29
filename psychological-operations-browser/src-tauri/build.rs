//! Tauri's `tauri_build::build()` + CEF runtime staging.
//!
//! `cef-dll-sys`'s build script extracts the upstream CEF binary
//! distribution into its `OUT_DIR` and emits the path via
//! `cargo:metadata=CEF_DIR=...`. Because cef-dll-sys declares
//! `links = "cef_dll_wrapper"` in its Cargo.toml, downstream
//! build scripts (this one) see the value as
//! `DEP_CEF_DLL_WRAPPER_CEF_DIR`.
//!
//! On every `cargo build`, this script copies the CEF runtime
//! files (libcef + paks + locales + GPU helpers) from that
//! extracted dir into `target/{profile}/` next to our binary so
//! `cargo build && ./target/debug/psychological-operations-browser ...`
//! works on a fresh checkout without manual staging.
//!
//! macOS is excluded — CEF on macOS lives inside an `.app`
//! bundle under `Contents/Frameworks/`, which is constructed by
//! a different pipeline (`bundle-cef-app` + Tauri's bundler).

use std::path::{Path, PathBuf};

fn main() {
    tauri_build::build();
    stage_cef_runtime();
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn stage_cef_runtime() {
    // Re-run when the upstream CEF dir's path changes (i.e. on
    // version bumps that move the cef-dll-sys hash).
    println!("cargo:rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");

    let cef_dir = match std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            println!(
                "cargo:warning=DEP_CEF_DLL_WRAPPER_CEF_DIR not set — \
                 skipping CEF runtime staging. The binary will fail \
                 to launch without libcef.dll alongside it."
            );
            return;
        }
    };

    let target_dir = match resolve_target_profile_dir() {
        Some(d) => d,
        None => {
            println!(
                "cargo:warning=couldn't resolve target/<profile>/ from \
                 OUT_DIR ({:?}); skipping CEF runtime staging.",
                std::env::var("OUT_DIR").ok(),
            );
            return;
        }
    };

    // Re-run if any source file changes (covers CEF version
    // bumps that re-extract the dist).
    println!("cargo:rerun-if-changed={}", cef_dir.display());

    for entry in cef_runtime_entries() {
        copy_if_newer(&cef_dir.join(entry), &target_dir.join(entry));
    }
}

#[cfg(target_os = "macos")]
fn stage_cef_runtime() {
    // macOS bundling is handled separately via bundle-cef-app
    // (see plan item #1). build.rs no-op here.
}

/// `OUT_DIR` is `target/{profile}/build/<crate>-<hash>/out` under
/// the standard Cargo layout. Walk up three components to land on
/// `target/{profile}/`. Sanity-check by comparing the basename
/// against `PROFILE` (which Cargo also sets, holding `debug` or
/// `release`).
fn resolve_target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    let profile_dir = out_dir.parent()?.parent()?.parent()?.to_path_buf();
    let profile = std::env::var("PROFILE").ok()?;
    if profile_dir.file_name().and_then(|s| s.to_str()) == Some(profile.as_str()) {
        Some(profile_dir)
    } else {
        None
    }
}

fn copy_if_newer(src: &Path, dst: &Path) {
    if !src.exists() {
        // Some Linux files aren't present in all CEF builds (e.g.
        // chrome-sandbox is optional). Silent skip — the runtime
        // will complain if it actually needed it.
        return;
    }
    let src_meta = match src.metadata() {
        Ok(m) => m,
        Err(_) => return,
    };
    if src_meta.is_dir() {
        // `locales/` directory — recurse.
        let _ = std::fs::create_dir_all(dst);
        if let Ok(read) = std::fs::read_dir(src) {
            for entry in read.flatten() {
                let name = entry.file_name();
                copy_if_newer(&src.join(&name), &dst.join(&name));
            }
        }
        return;
    }
    let needs_copy = match dst.metadata() {
        Ok(dst_meta) => match (src_meta.modified(), dst_meta.modified()) {
            (Ok(s), Ok(d)) => s > d,
            _ => true,
        },
        Err(_) => true,
    };
    if !needs_copy {
        return;
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::copy(src, dst) {
        println!(
            "cargo:warning=failed to stage CEF runtime file {} -> {}: {e}",
            src.display(),
            dst.display(),
        );
    }
}

#[cfg(target_os = "windows")]
fn cef_runtime_entries() -> &'static [&'static str] {
    &[
        // DLLs
        "libcef.dll",
        "chrome_elf.dll",
        "d3dcompiler_47.dll",
        "dxcompiler.dll",
        "dxil.dll",
        "libEGL.dll",
        "libGLESv2.dll",
        "vk_swiftshader.dll",
        "vulkan-1.dll",
        // Data / resource blobs
        "chrome_100_percent.pak",
        "chrome_200_percent.pak",
        "resources.pak",
        "icudtl.dat",
        "v8_context_snapshot.bin",
        "vk_swiftshader_icd.json",
        // Localized strings
        "locales",
    ]
}

#[cfg(target_os = "linux")]
fn cef_runtime_entries() -> &'static [&'static str] {
    &[
        // Shared objects
        "libcef.so",
        "libEGL.so",
        "libGLESv2.so",
        "libvk_swiftshader.so",
        "libvulkan.so.1",
        // Sandbox launcher (optional, present on most builds)
        "chrome-sandbox",
        // Data / resource blobs
        "chrome_100_percent.pak",
        "chrome_200_percent.pak",
        "resources.pak",
        "icudtl.dat",
        "v8_context_snapshot.bin",
        "snapshot_blob.bin",
        "vk_swiftshader_icd.json",
        // Localized strings
        "locales",
    ]
}

#[cfg(target_os = "macos")]
fn cef_runtime_entries() -> &'static [&'static str] {
    &[]
}
