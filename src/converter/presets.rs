use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

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
    max_height: Option<u32>,
    video_bitrate_kbps: Option<u32>,
    encoding_speed: Option<EncodingSpeed>,
    keep_audio_tracks: bool,
    keep_subtitle_tracks: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EncodingSpeed {
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

pub(super) fn ffmpeg_arguments(
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
