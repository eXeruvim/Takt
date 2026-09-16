use std::process::Command;

fn git_output(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    if let Some(reference) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }

    let commit = git_output(&["rev-parse", "HEAD"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let tracked_status = git_output(&["status", "--porcelain", "--untracked-files=no"])
        .unwrap_or_else(|| "unknown".to_owned());
    let tracked_clean = if tracked_status == "unknown" {
        "unknown"
    } else if tracked_status.is_empty() {
        "true"
    } else {
        "false"
    };
    println!("cargo:rustc-env=OXIDE_BUILD_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=OXIDE_BUILD_TRACKED_CLEAN={tracked_clean}");

    slint_build::compile("ui/app_window.slint")?;
    Ok(())
}
