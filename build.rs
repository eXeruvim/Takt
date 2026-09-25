use std::process::Command;

fn git_output(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // GetWindowSubclass is a named export of Common Controls v6. Without
        // an embedded activation manifest, Windows loads the legacy system DLL
        // and terminates the process before main with STATUS_ENTRYPOINT_NOT_FOUND.
        // Both link.exe and lld-link support these flags; apply them to tests too.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

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
