use std::path::Path;

use crate::ipc::{AudioTrackInfo, ChapterInfo, SubtitleTrackInfo};
use crate::player::limits::{UI_TEXT_BYTES, bounded_text};
use libmpv::Mpv;

const MAX_TRACKS: i64 = 256;
const MAX_CHAPTERS: i64 = 1_000;

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
    pub audio_codec: String,
    pub container: String,
    pub file_size_bytes: u64,
    pub duration_seconds: f64,
    pub average_bitrate_bps: f64,
    pub video_track_count: u32,
    pub audio_track_count: u32,
    pub subtitle_track_count: u32,
}

impl MediaInfo {
    pub fn read(mpv: &Mpv) -> Self {
        let file_size_bytes =
            non_negative_u64(mpv.get_property::<i64>("file-size").unwrap_or_default());
        let duration_seconds = mpv.get_property::<f64>("duration").unwrap_or_default();
        let video_tracks = read_tracks(mpv, "video")
            .into_iter()
            .filter(|track| !track.album_art)
            .count();
        let audio_tracks = read_tracks(mpv, "audio").len();
        let subtitle_tracks = read_tracks(mpv, "sub").len();

        Self {
            width: non_negative_u32(mpv.get_property::<i64>("width").unwrap_or_default()),
            height: non_negative_u32(mpv.get_property::<i64>("height").unwrap_or_default()),
            fps: mpv.get_property::<f64>("container-fps").unwrap_or_default(),
            codec: bounded_property(mpv, "current-tracks/video/codec", "video-format"),
            audio_codec: bounded_property(mpv, "current-tracks/audio/codec", "audio-codec"),
            container: bounded_text(
                &mpv.get_property::<String>("file-format")
                    .unwrap_or_default(),
                UI_TEXT_BYTES,
            ),
            file_size_bytes,
            duration_seconds,
            average_bitrate_bps: average_bitrate_bps(file_size_bytes, duration_seconds),
            video_track_count: u32::try_from(video_tracks).unwrap_or(u32::MAX),
            audio_track_count: u32::try_from(audio_tracks).unwrap_or(u32::MAX),
            subtitle_track_count: u32::try_from(subtitle_tracks).unwrap_or(u32::MAX),
        }
    }
}

pub fn read_audio_tracks(mpv: &Mpv) -> Vec<AudioTrackInfo> {
    read_tracks(mpv, "audio")
        .into_iter()
        .enumerate()
        .map(|(index, track)| AudioTrackInfo {
            id: track.id,
            label: track_label(
                "Дорожка",
                index + 1,
                &track.title,
                &track.language,
                &track.codec,
                None,
            ),
            selected: track.selected,
        })
        .collect()
}

pub fn read_subtitle_tracks(mpv: &Mpv) -> Vec<SubtitleTrackInfo> {
    read_tracks(mpv, "sub")
        .into_iter()
        .enumerate()
        .map(|(index, track)| {
            let external_title = track.external_filename.as_deref().and_then(|path| {
                Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
            let title = if track.title.trim().is_empty() {
                external_title.as_deref().unwrap_or_default()
            } else {
                &track.title
            };

            SubtitleTrackInfo {
                id: track.id,
                label: track_label(
                    "Субтитры",
                    index + 1,
                    title,
                    &track.language,
                    &track.codec,
                    track.external.then_some("Внешние"),
                ),
                selected: track.selected,
            }
        })
        .collect()
}

pub fn read_chapters(mpv: &Mpv) -> Vec<ChapterInfo> {
    let count = mpv
        .get_property::<i64>("chapter-list/count")
        .unwrap_or_default()
        .clamp(0, MAX_CHAPTERS);

    (0..count)
        .map(|index| {
            let title = bounded_text(
                &mpv.get_property::<String>(&format!("chapter-list/{index}/title"))
                    .unwrap_or_default(),
                UI_TEXT_BYTES,
            );
            let time = mpv
                .get_property::<f64>(&format!("chapter-list/{index}/time"))
                .unwrap_or_default();
            let title = if title.trim().is_empty() {
                format!("Глава {}", index + 1)
            } else {
                title
            };

            ChapterInfo {
                index,
                label: bounded_text(
                    &format!("{}  ·  {}", format_chapter_time(time), title),
                    UI_TEXT_BYTES,
                ),
            }
        })
        .collect()
}

fn format_chapter_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

struct RawTrack {
    id: i64,
    title: String,
    language: String,
    codec: String,
    selected: bool,
    external: bool,
    external_filename: Option<String>,
    album_art: bool,
}

fn read_tracks(mpv: &Mpv, track_type: &str) -> Vec<RawTrack> {
    let count = mpv
        .get_property::<i64>("track-list/count")
        .unwrap_or_default()
        .clamp(0, MAX_TRACKS);
    let mut tracks = Vec::new();

    for index in 0..count {
        let property = |name: &str| format!("track-list/{index}/{name}");
        if mpv
            .get_property::<String>(&property("type"))
            .unwrap_or_default()
            != track_type
        {
            continue;
        }

        let Ok(id) = mpv.get_property::<i64>(&property("id")) else {
            continue;
        };
        let title = bounded_text(
            &mpv.get_property::<String>(&property("title"))
                .unwrap_or_default(),
            UI_TEXT_BYTES,
        );
        let language = bounded_text(
            &mpv.get_property::<String>(&property("lang"))
                .unwrap_or_default(),
            UI_TEXT_BYTES,
        );
        let codec = bounded_text(
            &mpv.get_property::<String>(&property("codec"))
                .unwrap_or_default(),
            UI_TEXT_BYTES,
        );
        let selected = mpv
            .get_property::<bool>(&property("selected"))
            .unwrap_or(false);
        let external = mpv
            .get_property::<bool>(&property("external"))
            .unwrap_or(false);
        let external_filename = mpv
            .get_property::<String>(&property("external-filename"))
            .ok()
            .map(|value| bounded_text(&value, UI_TEXT_BYTES));
        let album_art = mpv
            .get_property::<bool>(&property("albumart"))
            .unwrap_or(false);

        tracks.push(RawTrack {
            id,
            title,
            language,
            codec,
            selected,
            external,
            external_filename,
            album_art,
        });
    }

    tracks
}

fn track_label(
    fallback: &str,
    position: usize,
    title: &str,
    language: &str,
    codec: &str,
    qualifier: Option<&str>,
) -> String {
    let title = title.trim();
    let language = language.trim();
    let codec = codec.trim();
    let mut label = if title.is_empty() {
        format!("{fallback} {position}")
    } else {
        title.to_owned()
    };
    let details = [
        (!language.is_empty()).then(|| language.to_uppercase()),
        (!codec.is_empty()).then(|| codec.to_uppercase()),
        qualifier.map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if !details.is_empty() {
        label.push_str("  ·  ");
        label.push_str(&details.join("  ·  "));
    }
    bounded_text(&label, UI_TEXT_BYTES)
}

fn bounded_property(mpv: &Mpv, primary: &str, fallback: &str) -> String {
    bounded_text(
        &mpv.get_property::<String>(primary)
            .or_else(|_| mpv.get_property::<String>(fallback))
            .unwrap_or_default(),
        UI_TEXT_BYTES,
    )
}

fn non_negative_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn non_negative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn average_bitrate_bps(file_size_bytes: u64, duration_seconds: f64) -> f64 {
    if file_size_bytes == 0 || !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        0.0
    } else {
        file_size_bytes as f64 * 8.0 / duration_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::{
        average_bitrate_bps, format_chapter_time, non_negative_u32, non_negative_u64, track_label,
    };

    #[test]
    fn rejects_negative_media_dimensions() {
        assert_eq!(non_negative_u32(-1), 0);
        assert_eq!(non_negative_u32(1_920), 1_920);
    }

    #[test]
    fn rejects_negative_file_sizes_and_calculates_average_bitrate() {
        assert_eq!(non_negative_u64(-1), 0);
        assert_eq!(non_negative_u64(1_048_576), 1_048_576);
        assert_eq!(average_bitrate_bps(1_000_000, 10.0), 800_000.0);
        assert_eq!(average_bitrate_bps(1_000_000, 0.0), 0.0);
        assert_eq!(average_bitrate_bps(0, 10.0), 0.0);
    }

    #[test]
    fn labels_audio_tracks_with_useful_fallbacks() {
        assert_eq!(
            track_label("Дорожка", 2, " Commentary ", "eng", "aac", None),
            "Commentary  ·  ENG  ·  AAC"
        );
        assert_eq!(
            track_label("Дорожка", 1, "", "rus", "", None),
            "Дорожка 1  ·  RUS"
        );
        assert_eq!(track_label("Дорожка", 3, "", "", "", None), "Дорожка 3");
        assert_eq!(
            track_label("Субтитры", 1, "captions.srt", "", "subrip", Some("Внешние")),
            "captions.srt  ·  SUBRIP  ·  Внешние"
        );
    }

    #[test]
    fn formats_chapter_start_times() {
        assert_eq!(format_chapter_time(65.2), "01:05");
        assert_eq!(format_chapter_time(3_661.0), "01:01:01");
        assert_eq!(format_chapter_time(-5.0), "00:00");
    }

    #[test]
    fn generated_track_labels_have_a_fixed_utf8_payload() {
        let label = track_label(
            "Дорожка",
            1,
            &"я".repeat(super::UI_TEXT_BYTES),
            &"language".repeat(1_000),
            &"codec".repeat(1_000),
            Some("Внешние"),
        );
        assert!(label.len() <= super::UI_TEXT_BYTES);
        assert!(label.ends_with('…'));
    }
}
