use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let chrome_dir = manifest_dir
        .parent()
        .unwrap()
        .join("psychological-operations-chrome");

    let target = env::var("TARGET").unwrap();
    let profile = if env::var("PROFILE").unwrap() == "release" { "release" } else { "debug" };

    // Validate the embedded chrome bundle.
    let validate_script = chrome_dir.join("validate.sh");
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
                "\n\npsychological-operations-chrome bundle is missing or stale.\n\
                 Run: bash psychological-operations-chrome/build.sh --target {target}{}\n\
                 Exit code: {}\n",
                if profile == "release" { " --release" } else { "" },
                s.code().unwrap_or(-1),
            );
        }
        Err(e) => {
            panic!("Failed to run validate.sh: {e}. Make sure bash is available.");
        }
    }

    let embed_dir = chrome_dir.join("embed").join(&target).join(profile);

    println!(
        "cargo:rustc-env=PSYOPS_CHROME_BUNDLE_PATH={}",
        embed_dir.join("chrome-bundle.zip").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_EXTENSION_TAR_PATH={}",
        embed_dir.join("extension.tar").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_EXTENSION_CRX_PATH={}",
        embed_dir.join("extension.crx").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_EXTENSION_ID_PATH={}",
        embed_dir.join("extension-id.txt").display(),
    );
    println!(
        "cargo:rustc-env=PSYOPS_CHROME_LAUNCH_ENTRY_PATH={}",
        embed_dir.join("launch-entry.txt").display(),
    );
    println!(
        "cargo:rerun-if-changed={}",
        chrome_dir.join("embed").display(),
    );
}
