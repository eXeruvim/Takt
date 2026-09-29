use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender};

#[cfg(test)]
mod lifecycle_tests;
mod output;
mod process;
mod queue;
use output::{WorkingOutput, publish_output};
use process::ManagedChild;
use queue::PendingJobs;

const ACTIVE_WORK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_ERROR_LOG_BYTES: usize = 32 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_OUTPUT_LIMIT: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024;
pub const COMMAND_CAPACITY: usize = 16;

#[derive(Default)]
pub struct ConverterControl {
    cancel: Arc<AtomicBool>,
    queue_generation: AtomicU64,
    refresh_pending: AtomicBool,
}

impl ConverterControl {
    pub fn cancel_active(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn clear_pending(&self) {
        self.queue_generation.fetch_add(1, Ordering::AcqRel);
    }
    pub fn queue_generation(&self) -> u64 {
        self.queue_generation.load(Ordering::Acquire)
    }

    pub fn request_refresh(&self, tx: &Sender<ConverterCommand>) -> Result<(), &'static str> {
        if self.refresh_pending.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Err(error) = submit(tx, ConverterCommand::RefreshAvailability) {
            self.refresh_pending.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }
}

pub fn submit(
    tx: &Sender<ConverterCommand>,
    command: ConverterCommand,
) -> Result<(), &'static str> {
    tx.try_send(command).map_err(|error| match error {
        crossbeam_channel::TrySendError::Full(_) => "Слишком много запросов к конвертеру. Подождите немного и повторите — задача не добавлена",
        crossbeam_channel::TrySendError::Disconnected(_) => "Конвертер недоступен. Перезапустите плеер",
    })
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConversionOptions {
    pub max_height: Option<u32>,
    pub video_bitrate_kbps: Option<u32>,
    pub encoding_speed: Option<EncodingSpeed>,
    pub keep_audio_tracks: bool,
    pub keep_subtitle_tracks: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingSpeed {
    Fast,
    Balanced,
    Thorough,
}

impl ConversionOptions {
    pub const fn from_ui_indices(
        resolution_index: i32,
        bitrate_index: i32,
        encoding_speed_index: i32,
        keep_audio_tracks: bool,
        keep_subtitle_tracks: bool,
    ) -> Self {
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
        let encoding_speed = match encoding_speed_index {
            1 => Some(EncodingSpeed::Fast),
            2 => Some(EncodingSpeed::Balanced),
            3 => Some(EncodingSpeed::Thorough),
            _ => None,
        };
        Self {
            max_height,
            video_bitrate_kbps,
            encoding_speed,
            keep_audio_tracks,
            keep_subtitle_tracks,
        }
    }
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            max_height: None,
            video_bitrate_kbps: None,
            encoding_speed: None,
            keep_audio_tracks: true,
            keep_subtitle_tracks: true,
        }
    }
}

#[derive(Debug)]
pub enum ConverterCommand {
    RefreshAvailability,
    Start {
        queue_generation: u64,
        source: PathBuf,
        output: PathBuf,
        preset: ConversionPreset,
        options: ConversionOptions,
        duration_seconds: f64,
    },
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
    Canceled {
        output: PathBuf,
    },
    QueueUpdated {
        pending: usize,
    },
    Rejected {
        output: PathBuf,
        message: String,
        active: bool,
    },
    Failed {
        output: PathBuf,
        message: String,
    },
}

#[derive(Debug)]
struct FfmpegInstallation {
    executable: PathBuf,
    details: String,
}

struct ActiveJob {
    output: PathBuf,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ActiveJob {
    fn is_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    fn finish(mut self, tx: &Sender<ConverterEvent>) {
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            let _ = tx.send(ConverterEvent::Failed {
                output: self.output.clone(),
                message: "Задача конвертации аварийно завершилась".to_owned(),
            });
        }
    }
}

impl Drop for ActiveJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!("Conversion worker panicked during shutdown");
        }
    }
}

/// Keep a bounded partial line even if a child never writes a newline.
#[derive(Default)]
struct OutputLines {
    pending: Vec<u8>,
}

impl OutputLines {
    fn feed(&mut self, bytes: &[u8], finished: bool) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                lines.push(
                    String::from_utf8_lossy(&self.pending)
                        .trim_end_matches('\r')
                        .to_owned(),
                );
                self.pending.clear();
            } else if self.pending.len() < MAX_LINE_BYTES {
                self.pending.push(byte);
            }
        }
        if finished && !self.pending.is_empty() {
            lines.push(String::from_utf8_lossy(&self.pending).into_owned());
            self.pending.clear();
        }
        lines
    }
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
}

pub fn run(
    rx: Receiver<ConverterCommand>,
    tx: Sender<ConverterEvent>,
    shutdown: Arc<AtomicBool>,
    control: Arc<ConverterControl>,
) {
    let mut installation = discover_ffmpeg(&shutdown);
    send_availability(&tx, installation.as_ref());
    let mut active_job: Option<ActiveJob> = None;
    let mut pending_jobs = PendingJobs::new(control.queue_generation());

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        if pending_jobs.synchronize(control.queue_generation()) {
            let _ = tx.send(ConverterEvent::QueueUpdated { pending: 0 });
        }
        if active_job.as_ref().is_some_and(ActiveJob::is_finished)
            && let Some(job) = active_job.take()
        {
            job.finish(&tx);
        }
        if active_job.is_none()
            && let Some(installation) = installation.as_ref()
            && let Some(job) = pending_jobs.pop()
        {
            let _ = tx.send(ConverterEvent::QueueUpdated {
                pending: pending_jobs.len(),
            });
            active_job = spawn_job(installation, job, &tx, control.cancel.clone());
        }

        // With no conversion running there is nothing to poll: sleep until a
        // real request arrives. During an active job the dispatcher still
        // wakes periodically to join it and start the next queued request.
        let command = if active_job.is_some() {
            rx.recv_timeout(ACTIVE_WORK_POLL_INTERVAL)
        } else {
            rx.recv()
                .map_err(|_| crossbeam_channel::RecvTimeoutError::Disconnected)
        };
        // Shutdown can race with a command that wakes the idle receiver. Do not
        // start new filesystem or FFmpeg work after the UI has begun exiting.
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match command {
            Ok(ConverterCommand::RefreshAvailability) => {
                if active_job.is_none() {
                    installation = discover_ffmpeg(&shutdown);
                    send_availability(&tx, installation.as_ref());
                }
                control.refresh_pending.store(false, Ordering::Release);
            }
            Ok(ConverterCommand::Start {
                queue_generation,
                source,
                output,
                preset,
                options,
                duration_seconds,
            }) => {
                let generation = control.queue_generation();
                if pending_jobs.synchronize(generation) {
                    let _ = tx.send(ConverterEvent::QueueUpdated { pending: 0 });
                }
                // Clear removes only requests submitted before it. New requests
                // remain valid even if still waiting in the bounded command queue.
                if queue_generation != generation {
                    continue;
                }
                let Some(installation) = installation.as_ref() else {
                    let _ = tx.send(ConverterEvent::Rejected {
                        output,
                        message: "FFmpeg не найден. Поместите ffmpeg.exe рядом с Oxide или добавьте его в PATH"
                            .to_owned(),
                        active: active_job.is_some(),
                    });
                    continue;
                };
                if let Err(message) = validate_job(&source, &output) {
                    let _ = tx.send(ConverterEvent::Rejected {
                        output,
                        message,
                        active: active_job.is_some(),
                    });
                    continue;
                }
                if active_job.as_ref().is_some_and(|job| job.output == output)
                    || pending_jobs.contains_output(&output)
                {
                    let _ = tx.send(ConverterEvent::Rejected {
                        message: format!(
                            "Этот выходной файл уже занят другой задачей: {}",
                            output.display()
                        ),
                        active: true,
                        output,
                    });
                    continue;
                }

                let job = JobSpec {
                    source,
                    output,
                    preset,
                    options,
                    duration_seconds,
                };
                if active_job.is_some() {
                    if let Err(job) = pending_jobs.push(job) {
                        let _ = tx.send(ConverterEvent::Rejected {
                            output: job.output,
                            message: format!("В очереди уже {} задач. Дождитесь завершения или очистите очередь — новая задача не добавлена", queue::MAX_PENDING_JOBS),
                            active: true,
                        });
                        continue;
                    }
                    let _ = tx.send(ConverterEvent::QueueUpdated {
                        pending: pending_jobs.len(),
                    });
                } else {
                    active_job = spawn_job(installation, job, &tx, control.cancel.clone());
                }
            }
            Ok(ConverterCommand::Quit) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn spawn_job(
    installation: &FfmpegInstallation,
    job: JobSpec,
    tx: &Sender<ConverterEvent>,
    cancel: Arc<AtomicBool>,
) -> Option<ActiveJob> {
    let output = job.output.clone();
    // UI cancellation bypasses the command/event queues. Reset only when a new
    // job takes ownership; the previous worker has already been joined.
    cancel.store(false, Ordering::Release);
    let executable = installation.executable.clone();
    let worker_tx = tx.clone();
    let worker_cancel = cancel.clone();
    let worker = thread::Builder::new()
        .name("oxide-conversion".into())
        .spawn(move || {
            run_job(&executable, job, &worker_cancel, &worker_tx);
        });
    match worker {
        Ok(worker) => Some(ActiveJob {
            output,
            cancel,
            worker: Some(worker),
        }),
        Err(error) => {
            let _ = tx.send(ConverterEvent::Failed {
                output,
                message: format!("Не удалось запустить задачу конвертации: {error}"),
            });
            None
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

fn discover_ffmpeg(shutdown: &AtomicBool) -> Option<FfmpegInstallation> {
    for candidate in ffmpeg_candidates() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let mut command = converter_process(&candidate);
        command.arg("-version");
        match probe_ffmpeg(command, shutdown, PROBE_TIMEOUT) {
            Ok(details) => {
                return Some(FfmpegInstallation {
                    executable: candidate,
                    details,
                });
            }
            Err(error) => eprintln!("FFmpeg probe {}: {error}", candidate.display()),
        }
    }
    None
}

fn probe_ffmpeg(
    command: Command,
    shutdown: &AtomicBool,
    timeout: Duration,
) -> std::io::Result<String> {
    use std::io::{Error, ErrorKind};
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
        thread::sleep(ACTIVE_WORK_POLL_INTERVAL);
    }
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
    let source = job.source.clone();
    let preset = job.preset;
    let options = job.options;
    run_job_with(job, cancel, tx, |working_output| {
        let mut command = converter_process(executable);
        command.args(ffmpeg_arguments(&source, working_output, preset, options));
        command
    });
}

fn run_job_with(
    job: JobSpec,
    cancel: &AtomicBool,
    tx: &Sender<ConverterEvent>,
    make_command: impl FnOnce(&Path) -> Command,
) {
    let JobSpec {
        source,
        output,
        duration_seconds,
        ..
    } = job;
    let source = source.as_path();
    let output = output.as_path();
    if let Err(message) = validate_job(source, output) {
        let _ = tx.send(ConverterEvent::Failed {
            output: output.to_path_buf(),
            message,
        });
        return;
    }

    let working_output = match WorkingOutput::new(output) {
        Ok(working_output) => working_output,
        Err(error) => {
            let _ = tx.send(ConverterEvent::Failed {
                output: output.to_path_buf(),
                message: format!("Не удалось подготовить временный файл: {error}"),
            });
            return;
        }
    };
    let source_size_bytes = fs::metadata(source)
        .map(|metadata| metadata.len())
        .unwrap_or_default();

    if cancel.load(Ordering::Acquire) {
        let _ = tx.send(ConverterEvent::Canceled {
            output: output.to_path_buf(),
        });
        return;
    }
    let mut child = match ManagedChild::spawn(make_command(&working_output.path)) {
        Ok(child) => child,
        Err(error) => {
            let _ = tx.send(ConverterEvent::Failed {
                output: output.to_path_buf(),
                message: format!("Не удалось запустить FFmpeg: {error}"),
            });
            return;
        }
    };

    let mut started = ConverterEvent::Started {
        output: output.to_path_buf(),
    };
    loop {
        match tx.send_timeout(started, ACTIVE_WORK_POLL_INTERVAL) {
            Ok(()) => break,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return,
            Err(crossbeam_channel::SendTimeoutError::Timeout(event)) => {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                started = event;
            }
        }
    }

    monitor_child(
        &mut child,
        JobMonitor {
            output,
            working_output: &working_output.path,
            duration_seconds,
            source_size_bytes,
            cancel,
            tx,
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

const fn x26x_preset(options: ConversionOptions, profile_default: &'static str) -> &'static str {
    match options.encoding_speed {
        Some(EncodingSpeed::Fast) => "veryfast",
        Some(EncodingSpeed::Balanced) => "medium",
        Some(EncodingSpeed::Thorough) => "slow",
        None => profile_default,
    }
}

fn append_vp9_encoding_speed(arguments: &mut Vec<OsString>, options: ConversionOptions) {
    let cpu_used = match options.encoding_speed {
        Some(EncodingSpeed::Fast) => Some("5"),
        Some(EncodingSpeed::Balanced) => Some("2"),
        Some(EncodingSpeed::Thorough) => Some("0"),
        None => None,
    };
    if let Some(cpu_used) = cpu_used {
        append_arguments(arguments, &["-deadline", "good", "-cpu-used", cpu_used]);
    }
}

fn append_video_stream_maps(arguments: &mut Vec<OsString>, options: ConversionOptions) {
    append_arguments(arguments, &["-map", "0:v:0?"]);
    if options.keep_audio_tracks {
        append_arguments(arguments, &["-map", "0:a?"]);
    }
    if options.keep_subtitle_tracks {
        append_arguments(arguments, &["-map", "0:s?"]);
    }
}

fn append_mp4_auxiliary_codecs(
    arguments: &mut Vec<OsString>,
    options: ConversionOptions,
    audio_bitrate: &str,
) {
    if options.keep_audio_tracks {
        append_arguments(arguments, &["-c:a", "aac", "-b:a", audio_bitrate]);
    }
    if options.keep_subtitle_tracks {
        append_arguments(arguments, &["-c:s", "mov_text"]);
    }
}

fn preset_arguments(preset: ConversionPreset, options: ConversionOptions) -> Vec<OsString> {
    let mut arguments = Vec::new();
    match preset {
        ConversionPreset::Mp4H264 => {
            let encoder_preset = x26x_preset(options, "medium");
            append_video_stream_maps(&mut arguments, options);
            append_arguments(
                &mut arguments,
                &["-c:v", "libx264", "-preset", encoder_preset],
            );
            append_rate_control(&mut arguments, "20", options, false);
            append_mp4_auxiliary_codecs(&mut arguments, options, "192k");
        }
        ConversionPreset::Mp4H265 => {
            let encoder_preset = x26x_preset(options, "medium");
            append_video_stream_maps(&mut arguments, options);
            append_arguments(
                &mut arguments,
                &["-c:v", "libx265", "-preset", encoder_preset],
            );
            append_rate_control(&mut arguments, "24", options, false);
            append_arguments(&mut arguments, &["-tag:v", "hvc1"]);
            append_mp4_auxiliary_codecs(&mut arguments, options, "192k");
        }
        ConversionPreset::WebM => {
            append_video_stream_maps(&mut arguments, options);
            append_arguments(&mut arguments, &["-c:v", "libvpx-vp9"]);
            append_vp9_encoding_speed(&mut arguments, options);
            append_rate_control(&mut arguments, "30", options, true);
            if options.keep_audio_tracks {
                append_arguments(&mut arguments, &["-c:a", "libopus", "-b:a", "128k"]);
            }
            if options.keep_subtitle_tracks {
                append_arguments(&mut arguments, &["-c:s", "webvtt"]);
            }
        }
        ConversionPreset::ExtractAudio => append_arguments(
            &mut arguments,
            &["-vn", "-map", "0:a:0?", "-c:a", "libmp3lame", "-q:a", "2"],
        ),
        ConversionPreset::SmallerFile => {
            let encoder_preset = x26x_preset(options, "slow");
            append_video_stream_maps(&mut arguments, options);
            append_arguments(
                &mut arguments,
                &["-c:v", "libx264", "-preset", encoder_preset],
            );
            append_rate_control(&mut arguments, "28", options, false);
            append_mp4_auxiliary_codecs(&mut arguments, options, "128k");
        }
        ConversionPreset::OriginalQuality => {
            append_arguments(&mut arguments, &["-map", "0", "-c", "copy"]);
            if !options.keep_audio_tracks {
                append_arguments(&mut arguments, &["-an"]);
            }
            if !options.keep_subtitle_tracks {
                append_arguments(&mut arguments, &["-sn"]);
            }
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

fn monitor_child(child: &mut ManagedChild, monitor: JobMonitor<'_>) {
    if let Err(message) = monitor_child_result(child, &monitor) {
        let cleanup_error = child.terminate().err();
        remove_generated_output(monitor.working_output);
        let _ = monitor.tx.send(ConverterEvent::Failed {
            output: monitor.output.to_path_buf(),
            message: match cleanup_error {
                Some(error) => format!("{message}\nНе удалось завершить FFmpeg: {error}"),
                None => message,
            },
        });
    }
}

fn monitor_child_result(child: &mut ManagedChild, monitor: &JobMonitor<'_>) -> Result<(), String> {
    let started_at = Instant::now();
    let mut error_log = String::new();
    let mut current_size_bytes = 0;
    let mut stdout_lines = OutputLines::default();
    let mut stderr_lines = OutputLines::default();

    loop {
        if monitor.cancel.load(Ordering::Acquire) {
            child
                .terminate()
                .map_err(|error| format!("Не удалось отменить FFmpeg: {error}"))?;
            remove_generated_output(monitor.working_output);
            let _ = monitor.tx.send(ConverterEvent::Canceled {
                output: monitor.output.to_path_buf(),
            });
            return Ok(());
        }

        let status = child
            .try_wait()
            .map_err(|error| format!("Не удалось получить состояние FFmpeg: {error}"))?;
        if status.is_some() {
            // End any inherited writer processes before reading the final pipe bytes.
            child
                .terminate()
                .map_err(|error| format!("Не удалось завершить FFmpeg: {error}"))?;
        }
        let stdout = child
            .stdout()
            .map_err(|error| format!("Ошибка чтения прогресса FFmpeg: {error}"))?;
        let stderr = child
            .stderr()
            .map_err(|error| format!("Ошибка чтения диагностики FFmpeg: {error}"))?;
        for line in stdout_lines.feed(&stdout, status.is_some()) {
            if let Some(bytes) = parse_progress_size(&line) {
                current_size_bytes = bytes;
            }
            if let Some(processed_seconds) = parse_progress_seconds(&line)
                && monitor.duration_seconds.is_finite()
                && monitor.duration_seconds > 0.0
            {
                let fraction = (processed_seconds / monitor.duration_seconds).clamp(0.0, 1.0);
                let remaining_seconds = estimate_remaining(started_at.elapsed(), fraction);
                let _ = monitor.tx.try_send(ConverterEvent::Progress {
                    fraction,
                    remaining_seconds,
                    current_size_bytes,
                });
            }
        }
        for line in stderr_lines.feed(&stderr, status.is_some()) {
            append_error_line(&mut error_log, &line);
        }

        match status {
            Some(status) if status.success() => {
                if let Err(error) = publish_output(monitor.working_output, monitor.output) {
                    remove_generated_output(monitor.working_output);
                    let _ = monitor.tx.send(ConverterEvent::Failed {
                        output: monitor.output.to_path_buf(),
                        message: if error.kind() == std::io::ErrorKind::AlreadyExists {
                            "Файл с выбранным именем появился во время конвертации. Он не изменён. Выберите другое имя и повторите конвертацию".to_owned()
                        } else {
                            format!("Не удалось сохранить готовый файл: {error}")
                        },
                    });
                    return Ok(());
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
                return Ok(());
            }
            Some(_) => {
                remove_generated_output(monitor.working_output);
                let message = last_error_lines(&error_log, 8);
                let _ = monitor.tx.send(ConverterEvent::Failed {
                    output: monitor.output.to_path_buf(),
                    message: if message.is_empty() {
                        "FFmpeg завершился с ошибкой".to_owned()
                    } else {
                        format!("FFmpeg завершился с ошибкой:\n{message}")
                    },
                });
                return Ok(());
            }
            None => thread::sleep(ACTIVE_WORK_POLL_INTERVAL),
        }
    }
}

fn remove_generated_output(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!(
            "Could not remove conversion temporary file {}: {error}",
            path.display()
        );
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
        "out_time_us" | "out_time_ms" => value
            .parse::<f64>()
            .ok()
            .map(|value| value / 1_000_000.0)
            .filter(|value| value.is_finite() && *value >= 0.0),
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
    let valid = parts.next().is_none()
        && hours.is_finite()
        && minutes.is_finite()
        && seconds.is_finite()
        && hours >= 0.0
        && (0.0..60.0).contains(&minutes)
        && (0.0..60.0).contains(&seconds);
    valid
        .then_some(hours * 3_600.0 + minutes * 60.0 + seconds)
        .filter(|value| value.is_finite())
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
    let remaining = MAX_ERROR_LOG_BYTES - log.len() - 1;
    let slice_end = line
        .char_indices()
        .take_while(|(index, character)| *index + character.len_utf8() <= remaining)
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
            ConversionOptions::from_ui_indices(3, 3, 1, true, true),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["-b:v", "8000k"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-map", "0:a?"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-map", "0:s?"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-c:s", "mov_text"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-preset", "veryfast"])
        );
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
            ConversionOptions::from_ui_indices(3, 2, 3, false, false),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["-c", "copy"]));
        assert!(!arguments.iter().any(|value| value == "-vf"));
        assert!(!arguments.iter().any(|value| value == "-b:v"));
        assert!(!arguments.iter().any(|value| value == "-preset"));
        assert!(arguments.iter().any(|value| value == "-an"));
        assert!(arguments.iter().any(|value| value == "-sn"));
    }

    #[test]
    fn maps_thorough_speed_to_vp9_encoder_effort() {
        let arguments = ffmpeg_arguments(
            Path::new("C:/video/input.mov"),
            Path::new("C:/video/output.webm"),
            ConversionPreset::WebM,
            ConversionOptions::from_ui_indices(0, 0, 3, true, true),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-deadline", "good"])
        );
        assert!(arguments.windows(2).any(|pair| pair == ["-cpu-used", "0"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-c:s", "webvtt"]));
    }

    #[test]
    fn removes_audio_and_subtitle_maps_from_reencoded_video() {
        let arguments = ffmpeg_arguments(
            Path::new("C:/video/input.mkv"),
            Path::new("C:/video/output.mp4"),
            ConversionPreset::Mp4H264,
            ConversionOptions::from_ui_indices(0, 0, 0, false, false),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

        assert!(!arguments.iter().any(|value| value == "0:a?"));
        assert!(!arguments.iter().any(|value| value == "0:s?"));
        assert!(!arguments.iter().any(|value| value == "-c:a"));
        assert!(!arguments.iter().any(|value| value == "-c:s"));
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
    fn rejects_non_finite_negative_and_malformed_progress_values() {
        for line in [
            "out_time_us=NaN",
            "out_time_us=inf",
            "out_time_us=-1",
            "out_time=NaN:00:00",
            "out_time=00:60:00",
            "out_time=00:00:60",
            "out_time=-1:00:00",
            "out_time=00:00:00:01",
            "out_time=not-a-time",
        ] {
            assert_eq!(parse_progress_seconds(line), None, "fixture: {line}");
        }
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
    fn keeps_only_requested_error_tail() {
        assert_eq!(last_error_lines("one\ntwo\nthree\n", 2), "two\nthree");
    }
}
