use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = manifest_dir.parent().unwrap();
    let target = env::var("TARGET").unwrap();
    let profile = if env::var("PROFILE").unwrap() == "release" { "release" } else { "debug" };

    browser_bundle(workspace_dir, &target, profile);
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
