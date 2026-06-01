use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = manifest_dir.parent().unwrap();
    let target = env::var("TARGET").unwrap();
    let profile = if env::var("PROFILE").unwrap() == "release" { "release" } else { "debug" };

    browser_bundle(workspace_dir, &target, profile);
    x_api_mcp_binary(workspace_dir, &target, profile);
}

fn browser_bundle(workspace_dir: &Path, target: &str, profile: &str) {
    let browser_dir = workspace_dir.join("psychological-operations-browser");
    let embed_dir = browser_dir.join("embed").join(target).join(profile);

    let bundle_path = embed_dir.join("browser-bundle.zip");
    let entry_path = embed_dir.join("browser-entry.txt");

    if !bundle_path.exists() || !entry_path.exists() {
        panic!(
            "\n\npsychological-operations-browser bundle missing at {}.\n\
             Run: pwsh psychological-operations-browser/scripts/build-bundle.ps1{}\n\
             (or scripts/build-bundle.sh on POSIX).\n",
            embed_dir.display(),
            if profile == "release" { " -Release" } else { "" },
        );
    }

    println!("cargo:rustc-env=PSYOPS_BROWSER_BUNDLE_PATH={}", bundle_path.display());
    println!("cargo:rustc-env=PSYOPS_BROWSER_ENTRY_PATH={}", entry_path.display());
    println!("cargo:rerun-if-changed={}", embed_dir.display());
}

/// Validate the pre-built `psychological-operations-x-api-mcp` binary
/// for the current target+profile exists at the expected embed path.
/// Refresh manually:
///     bash psychological-operations-x-api-mcp/build.sh [--release]
///
/// The fingerprint match between source and embed is enforced by
/// `build.sh` itself (it refuses to stamp the fingerprint until the
/// build succeeds). This build.rs only checks file existence — that
/// keeps the build.rs free of a bash dependency and works on Windows
/// without Git Bash on PATH.
fn x_api_mcp_binary(workspace_dir: &Path, target: &str, profile: &str) {
    let module = "psychological-operations-x-api-mcp";
    let binary_name = if target.contains("windows") {
        format!("{module}.exe")
    } else {
        module.to_string()
    };
    let embed_dir = workspace_dir.join(module).join("embed").join(target).join(profile);
    let binary_path = embed_dir.join(&binary_name);

    if !binary_path.exists() {
        panic!(
            "\n\n{module} embed binary missing at {}.\n\
             Run: bash {module}/build.sh{}\n",
            binary_path.display(),
            if profile == "release" { " --release" } else { "" },
        );
    }

    println!(
        "cargo:rustc-env=PSYOPS_X_API_MCP_BINARY_PATH={}",
        binary_path.display()
    );
    println!("cargo:rerun-if-changed={}", embed_dir.display());
}
