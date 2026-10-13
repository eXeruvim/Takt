use std::process::Command;

fn git_output(arguments: &[&str]) -> Option<String> {
    // Reading repository state from a build script must not refresh .git/index:
    // the index is itself a rerun trigger, so an optional refresh would force a
    // full relink on every otherwise identical Cargo invocation.
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Keep the executable identity, icon, DPI policy and Common Controls v6
        // activation in one resource so every Windows surface sees the same app.
        winresource::WindowsResource::new()
            .set_icon("assets/icons/oxide.ico")
            .set_manifest_file("assets/windows/app.manifest")
            .compile()?;
    }

    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=assets/icons/oxide.ico");
    println!("cargo:rerun-if-changed=assets/windows/app.manifest");
    for git_path in [".git/HEAD", ".git/index", ".git/packed-refs"] {
        if std::path::Path::new(git_path).exists() {
            println!("cargo:rerun-if-changed={git_path}");
        }
    }
    if let Some(reference) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
        let reference_path = format!(".git/{reference}");
        if std::path::Path::new(&reference_path).exists() {
            println!("cargo:rerun-if-changed={reference_path}");
        }
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
