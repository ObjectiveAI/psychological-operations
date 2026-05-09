use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let chromium_dir = manifest_dir
        .parent()
        .unwrap()
        .join("psychological-operations-chromium");

    let target = env::var("TARGET").unwrap();
    let profile = if env::var("PROFILE").unwrap() == "release" { "release" } else { "debug" };

    // Validate the embedded Chromium bundle.
    let validate_script = chromium_dir.join("validate.sh");
    let mut args: Vec<&str> = vec!["--target", &target];
    if profile == "release" {
        args.push("--release");
    }

    let status = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/c", "bash"])
            .arg(&validate_script)
            .args(&args)
            .status()
    } else {
        Command::new("bash")
            .arg(&validate_script)
            .args(&args)
            .status()
    };

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            panic!(
                "\n\npsychological-operations-chromium bundle is missing or stale.\n\
                 Run: bash psychological-operations-chromium/build.sh --target {target}{}\n\
                 Exit code: {}\n",
                if profile == "release" { " --release" } else { "" },
                s.code().unwrap_or(-1),
            );
        }
        Err(e) => {
            panic!("Failed to run validate.sh: {e}. Make sure bash is available.");
        }
    }

    let embed_dir = chromium_dir.join("embed").join(&target).join(profile);

    println!(
        "cargo:rustc-env=PSYOPS_CHROMIUM_BUNDLE_PATH={}",
        embed_dir.join("chromium-bundle.zip").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_EXTENSION_TAR_PATH={}",
        embed_dir.join("extension.tar").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_EXTENSION_ID_PATH={}",
        embed_dir.join("extension-id.txt").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_CHROMIUM_LAUNCH_ENTRY_PATH={}",
        embed_dir.join("launch-entry.txt").display(),
    );
    println!(
        "cargo:rerun-if-changed={}",
        chromium_dir.join("embed").display(),
    );
}
