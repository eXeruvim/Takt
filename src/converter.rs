use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, Sender, unbounded};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_ERROR_LOG_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversionPreset {
    Mp4H264,
    Mp4H265,
    WebM,
    ExtractAudio,
    SmallerFile,
    OriginalQuality,
    AudioAac,
    AudioFlac,
    AudioWav,
    AudioOpus,
}

impl ConversionPreset {
    pub const fn from_ui_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Mp4H264),
            1 => Some(Self::Mp4H265),
            2 => Some(Self::WebM),
            3 => Some(Self::ExtractAudio),
            4 => Some(Self::SmallerFile),
            5 => Some(Self::OriginalQuality),
            6 => Some(Self::AudioAac),
            7 => Some(Self::AudioFlac),
            8 => Some(Self::AudioWav),
            9 => Some(Self::AudioOpus),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mp4H264 | Self::Mp4H265 | Self::SmallerFile => "mp4",
            Self::WebM => "webm",
            Self::ExtractAudio => "mp3",
            Self::OriginalQuality => "mkv",
            Self::AudioAac => "m4a",
            Self::AudioFlac => "flac",
            Self::AudioWav => "wav",
            Self::AudioOpus => "opus",
        }
    }

    const fn supports_video_options(self) -> bool {
        matches!(
            self,
            Self::Mp4H264 | Self::Mp4H265 | Self::WebM | Self::SmallerFile
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConversionOptions {
    pub max_height: Option<u32>,
    pub video_bitrate_kbps: Option<u32>,
}

impl ConversionOptions {
    pub const fn from_ui_indices(resolution_index: i32, bitrate_index: i32) -> Self {
        let max_height = match resolution_index {
            1 => Some(2_160),
            2 => Some(1_440),
            3 => Some(1_080),
            4 => Some(720),
            5 => Some(480),
            _ => None,
        };
        let video_bitrate_kbps = match bitrate_index {
            1 => Some(2_000),
            2 => Some(4_000),
            3 => Some(8_000),
            4 => Some(12_000),
            5 => Some(20_000),
            _ => None,
        };
        Self {
            max_height,
            video_bitrate_kbps,
        }
    }
}

#[derive(Debug)]
pub enum ConverterCommand {
    RefreshAvailability,
    Start {
        source: PathBuf,
        output: PathBuf,
        preset: ConversionPreset,
        options: ConversionOptions,
        duration_seconds: f64,
    },
    Cancel,
    Quit,
}

#[derive(Clone, Debug)]
pub enum ConverterEvent {
    Availability {
        available: bool,
        details: String,
    },
    Started {
        output: PathBuf,
    },
    Progress {
        fraction: f64,
        remaining_seconds: Option<f64>,
        current_size_bytes: u64,
    },
    Completed {
        output: PathBuf,
        source_size_bytes: u64,
        output_size_bytes: u64,
    },
    Canceled,
    Failed(String),
}

#[derive(Debug)]
struct FfmpegInstallation {
    executable: PathBuf,
    details: String,
}

struct ActiveJob {
    cancel: Arc<AtomicBool>,
    finished: Receiver<()>,
}

struct JobSpec {
    source: PathBuf,
    output: PathBuf,
    preset: ConversionPreset,
    options: ConversionOptions,
    duration_seconds: f64,
}

struct JobMonitor<'a> {
    output: &'a Path,
    working_output: &'a Path,
    duration_seconds: f64,
    source_size_bytes: u64,
    cancel: &'a AtomicBool,
    tx: &'a Sender<ConverterEvent>,
    stdout_rx: Receiver<String>,
    stderr_rx: Receiver<String>,
}

pub fn run(rx: Receiver<ConverterCommand>, tx: Sender<ConverterEvent>) {
    let mut installation = discover_ffmpeg();
    send_availability(&tx, installation.as_ref());
    let mut active_job: Option<ActiveJob> = None;

    loop {
        if active_job
            .as_ref()
            .is_some_and(|job| job.finished.try_recv().is_ok())
        {
            active_job = None;
        }

        match rx.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(ConverterCommand::RefreshAvailability) => {
                if active_job.is_none() {
                    installation = discover_ffmpeg();
                    send_availability(&tx, installation.as_ref());
                }
            }
            Ok(ConverterCommand::Start {
                source,
                output,
                preset,
                options,
                duration_seconds,
            }) => {
                if active_job.is_some() {
                    let _ = tx.send(ConverterEvent::Failed(
                        "Другая конвертация уже выполняется".to_owned(),
                    ));
                    continue;
                }
                let Some(installation) = installation.as_ref() else {
                    let _ = tx.send(ConverterEvent::Failed(
                        "FFmpeg не найден. Поместите ffmpeg.exe рядом с Oxide или добавьте его в PATH"
                            .to_owned(),
                    ));
                    continue;
                };

                let cancel = Arc::new(AtomicBool::new(false));
                let (finished_tx, finished_rx) = unbounded();
                let executable = installation.executable.clone();
                let worker_tx = tx.clone();
                let worker_cancel = cancel.clone();
                thread::spawn(move || {
                    run_job(
                        &executable,
                        JobSpec {
                            source,
                            output,
                            preset,
                            options,
                            duration_seconds,
                        },
                        &worker_cancel,
                        &worker_tx,
                    );
                    let _ = finished_tx.send(());
                });
                active_job = Some(ActiveJob {
                    cancel,
                    finished: finished_rx,
                });
            }
            Ok(ConverterCommand::Cancel) => {
                if let Some(job) = &active_job {
                    job.cancel.store(true, Ordering::Release);
                }
            }
            Ok(ConverterCommand::Quit) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                if let Some(job) = &active_job {
                    job.cancel.store(true, Ordering::Release);
                    let _ = job.finished.recv_timeout(Duration::from_secs(5));
                }
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn send_availability(tx: &Sender<ConverterEvent>, installation: Option<&FfmpegInstallation>) {
    let (available, details) = installation
        .map(|installation| (true, installation.details.clone()))
        .unwrap_or_else(|| {
            (
                false,
                "FFmpeg не найден: добавьте его в PATH или положите ffmpeg.exe рядом с Oxide"
                    .to_owned(),
            )
        });
    let _ = tx.send(ConverterEvent::Availability { available, details });
}

fn discover_ffmpeg() -> Option<FfmpegInstallation> {
    ffmpeg_candidates().into_iter().find_map(|candidate| {
        let output = converter_process(&candidate)
            .arg("-version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let version = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("FFmpeg обнаружен")
            .trim()
            .to_owned();
        Some(FfmpegInstallation {
            details: version,
            executable: candidate,
        })
    })
}

fn ffmpeg_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("OXIDE_FFMPEG") {
        candidates.push(PathBuf::from(configured));
    }
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(directory) = current_exe.parent()
    {
        candidates.push(directory.join(ffmpeg_binary_name()));
        candidates.push(directory.join("tools").join(ffmpeg_binary_name()));
    }
    candidates.push(PathBuf::from(ffmpeg_binary_name()));
    candidates
}

const fn ffmpeg_binary_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn run_job(executable: &Path, job: JobSpec, cancel: &AtomicBool, tx: &Sender<ConverterEvent>) {
    let JobSpec {
        source,
        output,
        preset,
        options,
        duration_seconds,
    } = job;
    let source = source.as_path();
    let output = output.as_path();
    if let Err(message) = validate_job(source, output) {
        let _ = tx.send(ConverterEvent::Failed(message));
        return;
    }

    let working_output = working_output_path(output);
    let source_size_bytes = fs::metadata(source)
        .map(|metadata| metadata.len())
        .unwrap_or_default();

    let mut command = converter_process(executable);
    command
        .args(ffmpeg_arguments(source, &working_output, preset, options))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = tx.send(ConverterEvent::Failed(format!(
                "Не удалось запустить FFmpeg: {error}"
            )));
            return;
        }
    };

    let _ = tx.send(ConverterEvent::Started {
        output: output.to_path_buf(),
    });

    let (stdout_tx, stdout_rx) = unbounded();
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = stdout_tx.send(line);
            }
        });
    }

    let (stderr_tx, stderr_rx) = unbounded();
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = stderr_tx.send(line);
            }
        });
    }

    monitor_child(
        &mut child,
        JobMonitor {
            output,
            working_output: &working_output,
            duration_seconds,
            source_size_bytes,
            cancel,
            tx,
            stdout_rx,
            stderr_rx,
        },
    );
}

fn append_arguments(arguments: &mut Vec<OsString>, values: &[&str]) {
    arguments.extend(values.iter().map(OsString::from));
}

fn append_rate_control(
    arguments: &mut Vec<OsString>,
    default_crf: &str,
    options: ConversionOptions,
    constant_quality_requires_zero_bitrate: bool,
) {
    if let Some(bitrate) = options.video_bitrate_kbps {
        append_arguments(arguments, &["-b:v"]);
        arguments.push(format!("{bitrate}k").into());
    } else {
        append_arguments(arguments, &["-crf", default_crf]);
        if constant_quality_requires_zero_bitrate {
            append_arguments(arguments, &["-b:v", "0"]);
        }
    }
}

fn preset_arguments(preset: ConversionPreset, options: ConversionOptions) -> Vec<OsString> {
    let mut arguments = Vec::new();
    match preset {
        ConversionPreset::Mp4H264 => {
            append_arguments(
                &mut arguments,
                &[
                    "-map", "0:v:0?", "-map", "0:a?", "-c:v", "libx264", "-preset", "medium",
                ],
            );
            append_rate_control(&mut arguments, "20", options, false);
            append_arguments(&mut arguments, &["-c:a", "aac", "-b:a", "192k"]);
        }
        ConversionPreset::Mp4H265 => {
            append_arguments(
                &mut arguments,
                &[
                    "-map", "0:v:0?", "-map", "0:a?", "-c:v", "libx265", "-preset", "medium",
                ],
            );
            append_rate_control(&mut arguments, "24", options, false);
            append_arguments(
                &mut arguments,
                &["-tag:v", "hvc1", "-c:a", "aac", "-b:a", "192k"],
            );
        }
        ConversionPreset::WebM => {
            append_arguments(
                &mut arguments,
                &["-map", "0:v:0?", "-map", "0:a?", "-c:v", "libvpx-vp9"],
            );
            append_rate_control(&mut arguments, "30", options, true);
            append_arguments(&mut arguments, &["-c:a", "libopus", "-b:a", "128k"]);
        }
        ConversionPreset::ExtractAudio => append_arguments(
            &mut arguments,
            &["-vn", "-map", "0:a:0?", "-c:a", "libmp3lame", "-q:a", "2"],
        ),
        ConversionPreset::SmallerFile => {
            append_arguments(
                &mut arguments,
                &[
                    "-map", "0:v:0?", "-map", "0:a?", "-c:v", "libx264", "-preset", "slow",
                ],
            );
            append_rate_control(&mut arguments, "28", options, false);
            append_arguments(&mut arguments, &["-c:a", "aac", "-b:a", "128k"]);
        }
        ConversionPreset::OriginalQuality => {
            append_arguments(&mut arguments, &["-map", "0", "-c", "copy"]);
        }
        ConversionPreset::AudioAac => append_arguments(
            &mut arguments,
            &["-vn", "-map", "0:a:0?", "-c:a", "aac", "-b:a", "192k"],
        ),
        ConversionPreset::AudioFlac => append_arguments(
            &mut arguments,
            &[
                "-vn",
                "-map",
                "0:a:0?",
                "-c:a",
                "flac",
                "-compression_level",
                "8",
            ],
        ),
        ConversionPreset::AudioWav => append_arguments(
            &mut arguments,
            &["-vn", "-map", "0:a:0?", "-c:a", "pcm_s16le"],
        ),
        ConversionPreset::AudioOpus => append_arguments(
            &mut arguments,
            &["-vn", "-map", "0:a:0?", "-c:a", "libopus", "-b:a", "128k"],
        ),
    }

    if preset.supports_video_options()
        && let Some(max_height) = options.max_height
    {
        append_arguments(&mut arguments, &["-vf"]);
        arguments.push(format!("scale=-2:min(ih\\,{max_height})").into());
    }
    arguments
}

fn ffmpeg_arguments(
    source: &Path,
    output: &Path,
    preset: ConversionPreset,
    options: ConversionOptions,
) -> Vec<OsString> {
    let mut arguments = ["-nostdin", "-hide_banner", "-n", "-i"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    arguments.push(source.as_os_str().to_owned());
    arguments.extend(preset_arguments(preset, options));
    arguments.extend(["-progress", "pipe:1", "-nostats"].map(OsString::from));
    arguments.push(output.as_os_str().to_owned());
    arguments
}

fn monitor_child(child: &mut Child, monitor: JobMonitor<'_>) {
    let started_at = Instant::now();
    let mut error_log = String::new();
    let mut current_size_bytes = 0;

    loop {
        if monitor.cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            remove_generated_output(monitor.working_output);
            let _ = monitor.tx.send(ConverterEvent::Canceled);
            return;
        }

        for line in monitor.stdout_rx.try_iter() {
            if let Some(bytes) = parse_progress_size(&line) {
                current_size_bytes = bytes;
            }
            if let Some(processed_seconds) = parse_progress_seconds(&line)
                && monitor.duration_seconds.is_finite()
                && monitor.duration_seconds > 0.0
            {
                let fraction = (processed_seconds / monitor.duration_seconds).clamp(0.0, 1.0);
                let remaining_seconds = estimate_remaining(started_at.elapsed(), fraction);
                let _ = monitor.tx.send(ConverterEvent::Progress {
                    fraction,
                    remaining_seconds,
                    current_size_bytes,
                });
            }
        }
        for line in monitor.stderr_rx.try_iter() {
            append_error_line(&mut error_log, &line);
        }

        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                if let Err(error) = fs::rename(monitor.working_output, monitor.output) {
                    remove_generated_output(monitor.working_output);
                    let _ = monitor.tx.send(ConverterEvent::Failed(format!(
                        "Не удалось сохранить готовый файл: {error}"
                    )));
                    return;
                }
                let _ = monitor.tx.send(ConverterEvent::Progress {
                    fraction: 1.0,
                    remaining_seconds: Some(0.0),
                    current_size_bytes: fs::metadata(monitor.output)
                        .map(|metadata| metadata.len())
                        .unwrap_or(current_size_bytes),
                });
                let output_size_bytes = fs::metadata(monitor.output)
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let _ = monitor.tx.send(ConverterEvent::Completed {
                    output: monitor.output.to_path_buf(),
                    source_size_bytes: monitor.source_size_bytes,
                    output_size_bytes,
                });
                return;
            }
            Ok(Some(_)) => {
                remove_generated_output(monitor.working_output);
                for line in monitor.stderr_rx.try_iter() {
                    append_error_line(&mut error_log, &line);
                }
                let message = last_error_lines(&error_log, 8);
                let _ = monitor
                    .tx
                    .send(ConverterEvent::Failed(if message.is_empty() {
                        "FFmpeg завершился с ошибкой".to_owned()
                    } else {
                        format!("FFmpeg завершился с ошибкой:\n{message}")
                    }));
                return;
            }
            Ok(None) => thread::sleep(WORKER_POLL_INTERVAL),
            Err(error) => {
                remove_generated_output(monitor.working_output);
                let _ = monitor.tx.send(ConverterEvent::Failed(format!(
                    "Не удалось получить состояние FFmpeg: {error}"
                )));
                return;
            }
        }
    }
}

fn working_output_path(output: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let stem = output
        .file_stem()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let extension = output
        .extension()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let file_name = if extension.is_empty() {
        format!(".{stem}.oxide-part-{nonce}")
    } else {
        format!(".{stem}.oxide-part-{nonce}.{extension}")
    };
    output.with_file_name(file_name)
}

fn remove_generated_output(path: &Path) {
    if path.is_file() {
        let _ = fs::remove_file(path);
    }
}

fn validate_job(source: &Path, output: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("Исходный файл не найден: {}", source.display()));
    }
    if output.exists() {
        return Err(format!(
            "Выходной файл уже существует: {}. Выберите другое имя",
            output.display()
        ));
    }
    if source == output {
        return Err("Исходный и выходной файлы не могут совпадать".to_owned());
    }
    let Some(parent) = output.parent() else {
        return Err("Не удалось определить папку выходного файла".to_owned());
    };
    if !parent.is_dir() {
        return Err(format!("Папка не существует: {}", parent.display()));
    }
    Ok(())
}

fn parse_progress_seconds(line: &str) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => value.parse::<f64>().ok().map(|value| value / 1_000_000.0),
        "out_time" => parse_ffmpeg_timestamp(value),
        _ => None,
    }
}

fn parse_progress_size(line: &str) -> Option<u64> {
    let (key, value) = line.trim().split_once('=')?;
    (key == "total_size")
        .then(|| value.parse::<u64>().ok())
        .flatten()
}

fn parse_ffmpeg_timestamp(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    (parts.next().is_none() && hours >= 0.0 && minutes >= 0.0 && seconds >= 0.0)
        .then_some(hours * 3_600.0 + minutes * 60.0 + seconds)
}

fn estimate_remaining(elapsed: Duration, fraction: f64) -> Option<f64> {
    if !(0.01..1.0).contains(&fraction) {
        return None;
    }
    Some((elapsed.as_secs_f64() * (1.0 - fraction) / fraction).max(0.0))
}

fn append_error_line(log: &mut String, line: &str) {
    if log.len() >= MAX_ERROR_LOG_BYTES {
        return;
    }
    let remaining = MAX_ERROR_LOG_BYTES - log.len();
    let slice_end = line
        .char_indices()
        .take_while(|(index, _)| *index < remaining)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    log.push_str(&line[..slice_end]);
    log.push('\n');
}

fn last_error_lines(log: &str, count: usize) -> String {
    let lines = log.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

fn converter_process(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    configure_hidden_process(&mut command);
    command
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

pub fn suggested_output_path(source: &Path, preset: ConversionPreset) -> Option<PathBuf> {
    let parent = source.parent()?;
    let stem = source.file_stem()?.to_string_lossy();
    let suffix = match preset {
        ConversionPreset::Mp4H264 => "h264",
        ConversionPreset::Mp4H265 => "h265",
        ConversionPreset::WebM => "webm",
        ConversionPreset::ExtractAudio => "audio",
        ConversionPreset::SmallerFile => "small",
        ConversionPreset::OriginalQuality => "copy",
        ConversionPreset::AudioAac => "aac",
        ConversionPreset::AudioFlac => "flac",
        ConversionPreset::AudioWav => "wav",
        ConversionPreset::AudioOpus => "opus",
    };
    Some(parent.join(format!("{stem}-{suffix}.{}", preset.extension())))
}

pub fn output_filter(preset: ConversionPreset) -> (&'static str, &'static [&'static str]) {
    match preset {
        ConversionPreset::Mp4H264 | ConversionPreset::Mp4H265 | ConversionPreset::SmallerFile => {
            ("MP4", &["mp4"])
        }
        ConversionPreset::WebM => ("WebM", &["webm"]),
        ConversionPreset::ExtractAudio => ("MP3", &["mp3"]),
        ConversionPreset::OriginalQuality => ("Matroska", &["mkv"]),
        ConversionPreset::AudioAac => ("M4A / AAC", &["m4a"]),
        ConversionPreset::AudioFlac => ("FLAC", &["flac"]),
        ConversionPreset::AudioWav => ("WAV", &["wav"]),
        ConversionPreset::AudioOpus => ("Opus", &["opus"]),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConversionOptions, ConversionPreset, estimate_remaining, ffmpeg_arguments,
        last_error_lines, parse_progress_seconds, parse_progress_size, suggested_output_path,
        working_output_path,
    };
    use std::{path::Path, time::Duration};

    #[test]
    fn maps_presets_to_stable_ui_indices_and_extensions() {
        assert_eq!(
            ConversionPreset::from_ui_index(0),
            Some(ConversionPreset::Mp4H264)
        );
        assert_eq!(
            ConversionPreset::from_ui_index(5),
            Some(ConversionPreset::OriginalQuality)
        );
        assert_eq!(
            ConversionPreset::from_ui_index(9),
            Some(ConversionPreset::AudioOpus)
        );
        assert_eq!(ConversionPreset::from_ui_index(10), None);
        assert_eq!(ConversionPreset::ExtractAudio.extension(), "mp3");
        assert_eq!(ConversionPreset::AudioFlac.extension(), "flac");
    }

    #[test]
    fn builds_safe_h264_command_arguments() {
        let arguments = ffmpeg_arguments(
            Path::new("C:/video/input file.mov"),
            Path::new("C:/video/.output.oxide-part-1.mp4"),
            ConversionPreset::Mp4H264,
            ConversionOptions::default(),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(arguments.starts_with(&[
            "-nostdin".to_owned(),
            "-hide_banner".to_owned(),
            "-n".to_owned(),
            "-i".to_owned(),
            "C:/video/input file.mov".to_owned(),
        ]));
        assert!(arguments.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-progress", "pipe:1"])
        );
        assert_eq!(
            arguments.last().map(String::as_str),
            Some("C:/video/.output.oxide-part-1.mp4")
        );
    }

    #[test]
    fn applies_resolution_and_explicit_video_bitrate_without_crf() {
        let arguments = ffmpeg_arguments(
            Path::new("C:/video/input.mov"),
            Path::new("C:/video/output.mp4"),
            ConversionPreset::Mp4H265,
            ConversionOptions::from_ui_indices(3, 3),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["-b:v", "8000k"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-vf", "scale=-2:min(ih\\,1080)"])
        );
        assert!(!arguments.iter().any(|value| value == "-crf"));
    }

    #[test]
    fn ignores_video_options_for_stream_copy() {
        let arguments = ffmpeg_arguments(
            Path::new("C:/video/input.mov"),
            Path::new("C:/video/output.mkv"),
            ConversionPreset::OriginalQuality,
            ConversionOptions::from_ui_indices(3, 2),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["-c", "copy"]));
        assert!(!arguments.iter().any(|value| value == "-vf"));
        assert!(!arguments.iter().any(|value| value == "-b:v"));
    }

    #[test]
    fn parses_ffmpeg_progress_formats() {
        assert_eq!(parse_progress_seconds("out_time_us=2500000"), Some(2.5));
        assert_eq!(parse_progress_seconds("out_time_ms=2500000"), Some(2.5));
        assert_eq!(
            parse_progress_seconds("out_time=01:02:03.500000"),
            Some(3_723.5)
        );
        assert_eq!(parse_progress_seconds("speed=1.0x"), None);
        assert_eq!(parse_progress_size("total_size=1048576"), Some(1_048_576));
        assert_eq!(parse_progress_size("speed=1.0x"), None);
    }

    #[test]
    fn estimates_remaining_time_after_initial_progress() {
        assert_eq!(estimate_remaining(Duration::from_secs(10), 0.5), Some(10.0));
        assert_eq!(estimate_remaining(Duration::from_secs(10), 0.0), None);
        assert_eq!(estimate_remaining(Duration::from_secs(10), 1.0), None);
    }

    #[test]
    fn suggests_non_destructive_output_name() {
        assert_eq!(
            suggested_output_path(Path::new("C:/video/demo.mov"), ConversionPreset::Mp4H264),
            Some(Path::new("C:/video/demo-h264.mp4").to_path_buf())
        );
    }

    #[test]
    fn temporary_output_keeps_the_container_extension() {
        let path = working_output_path(Path::new("C:/video/demo.mp4"));
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("mp4")
        );
        assert!(
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with(".demo.oxide-part-"))
        );
    }

    #[test]
    fn keeps_only_requested_error_tail() {
        assert_eq!(last_error_lines("one\ntwo\nthree\n", 2), "two\nthree");
    }
}
