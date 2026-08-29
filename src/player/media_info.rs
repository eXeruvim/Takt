use std::path::Path;

use crate::ipc::{AudioTrackInfo, SubtitleTrackInfo};
use libmpv::Mpv;

#[derive(Debug, Default)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
    pub audio_codec: String,
}

impl MediaInfo {
    pub fn read(mpv: &Mpv) -> Self {
        Self {
            width: non_negative_u32(mpv.get_property::<i64>("width").unwrap_or_default()),
            height: non_negative_u32(mpv.get_property::<i64>("height").unwrap_or_default()),
            fps: mpv.get_property::<f64>("container-fps").unwrap_or_default(),
            codec: mpv
                .get_property::<String>("video-format")
                .unwrap_or_default(),
            audio_codec: mpv
                .get_property::<String>("audio-codec")
                .unwrap_or_default(),
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

struct RawTrack {
    id: i64,
    title: String,
    language: String,
    codec: String,
    selected: bool,
    external: bool,
    external_filename: Option<String>,
}

fn read_tracks(mpv: &Mpv, track_type: &str) -> Vec<RawTrack> {
    let count = mpv
        .get_property::<i64>("track-list/count")
        .unwrap_or_default()
        .max(0);
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
        let title = mpv
            .get_property::<String>(&property("title"))
            .unwrap_or_default();
        let language = mpv
            .get_property::<String>(&property("lang"))
            .unwrap_or_default();
        let codec = mpv
            .get_property::<String>(&property("codec"))
            .unwrap_or_default();
        let selected = mpv
            .get_property::<bool>(&property("selected"))
            .unwrap_or(false);
        let external = mpv
            .get_property::<bool>(&property("external"))
            .unwrap_or(false);
        let external_filename = mpv
            .get_property::<String>(&property("external-filename"))
            .ok();

        tracks.push(RawTrack {
            id,
            title,
            language,
            codec,
            selected,
            external,
            external_filename,
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
    label
}

fn non_negative_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{non_negative_u32, track_label};

    #[test]
    fn rejects_negative_media_dimensions() {
        assert_eq!(non_negative_u32(-1), 0);
        assert_eq!(non_negative_u32(1_920), 1_920);
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
}
