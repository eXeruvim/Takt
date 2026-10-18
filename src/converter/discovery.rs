use std::{
    ffi::OsString,
    io::{Error, ErrorKind},
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use super::process::{ManagedChild, command as ffmpeg_process};

const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug)]
pub(super) struct FfmpegInstallation {
    pub(super) executable: PathBuf,
    pub(super) details: String,
}

pub(super) fn discover(shutdown: &AtomicBool) -> Option<FfmpegInstallation> {
    let configured = std::env::var_os("OXIDE_FFMPEG");
    let current_exe = std::env::current_exe().ok();
    for candidate in candidates(configured, current_exe) {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let mut command = ffmpeg_process(&candidate);
        command.arg("-version");
        match probe(command, shutdown, PROBE_TIMEOUT) {
            Ok(details) => {
                return Some(FfmpegInstallation {
                    executable: candidate,
                    details,
                });
            }
            // A missing optional FFmpeg executable is a normal state: playback
            // uses the bundled libmpv runtime and the converter explains how to
            // provide FFmpeg only when the user opens it. Keep individual probe
            // failures available to developers without alarming release users.
            Err(error) => {
                #[cfg(debug_assertions)]
                eprintln!("FFmpeg probe {}: {error}", candidate.display());
                #[cfg(not(debug_assertions))]
                let _ = error;
            }
        }
    }
    None
}

pub(super) fn probe(
    command: Command,
    shutdown: &AtomicBool,
    timeout: Duration,
) -> std::io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut child = ManagedChild::spawn(command)?;
    let mut output = Vec::new();
    let mut total_bytes: usize = 0;
    loop {
        if shutdown.load(Ordering::Acquire) {
            return Err(ErrorKind::Interrupted.into());
        }
        if Instant::now() >= deadline {
            return Err(ErrorKind::TimedOut.into());
        }
        let status = child.try_wait()?;
        let stdout = child.stdout()?;
        let stderr = child.stderr()?;
        total_bytes = total_bytes
            .saturating_add(stdout.len())
            .saturating_add(stderr.len());
        if total_bytes > PROBE_OUTPUT_LIMIT {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "FFmpeg probe output limit exceeded",
            ));
        }
        let had_output = !stdout.is_empty() || !stderr.is_empty();
        output.extend(stdout);
        if let Some(status) = status {
            if !status.success() {
                return Err(Error::other("FFmpeg version probe failed"));
            }
            if had_output {
                continue;
            }
            return String::from_utf8_lossy(&output)
                .lines()
                .find(|line| line.starts_with("ffmpeg version "))
                .map(|line| line.chars().take(512).collect())
                .ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "FFmpeg version header missing")
                });
        }
        thread::sleep(PROBE_POLL_INTERVAL);
    }
}

fn candidates(configured: Option<OsString>, current_exe: Option<PathBuf>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = configured {
        candidates.push(PathBuf::from(configured));
    }
    if let Some(directory) = current_exe.as_deref().and_then(std::path::Path::parent) {
        candidates.push(directory.join(binary_name()));
        candidates.push(directory.join("tools").join(binary_name()));
    }
    candidates.push(PathBuf::from(binary_name()));
    candidates
}

const fn binary_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_prefer_configuration_then_application_and_path() {
        let configured = PathBuf::from("configured-ffmpeg");
        let executable = PathBuf::from("application").join("oxide");
        let candidates = candidates(Some(configured.clone().into_os_string()), Some(executable));

        assert_eq!(candidates[0], configured);
        assert_eq!(
            candidates[1],
            PathBuf::from("application").join(binary_name())
        );
        assert_eq!(
            candidates[2],
            PathBuf::from("application")
                .join("tools")
                .join(binary_name())
        );
        assert_eq!(candidates[3], PathBuf::from(binary_name()));
    }

    #[test]
    fn candidates_work_without_an_application_path_or_override() {
        assert_eq!(candidates(None, None), vec![PathBuf::from(binary_name())]);
    }
}
