use crate::ipc::AudioTrackInfo;
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
    let count = mpv
        .get_property::<i64>("track-list/count")
        .unwrap_or_default()
        .max(0);
    let mut audio_position = 0;
    let mut tracks = Vec::new();

    for index in 0..count {
        let property = |name: &str| format!("track-list/{index}/{name}");
        if mpv
            .get_property::<String>(&property("type"))
            .unwrap_or_default()
            != "audio"
        {
            continue;
        }

        let Ok(id) = mpv.get_property::<i64>(&property("id")) else {
            continue;
        };
        audio_position += 1;
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

        tracks.push(AudioTrackInfo {
            id,
            label: audio_track_label(audio_position, &title, &language, &codec),
            selected,
        });
    }

    tracks
}

fn audio_track_label(position: usize, title: &str, language: &str, codec: &str) -> String {
    let title = title.trim();
    let language = language.trim();
    let codec = codec.trim();
    let mut label = if title.is_empty() {
        format!("Дорожка {position}")
    } else {
        title.to_owned()
    };
    let details = [
        (!language.is_empty()).then(|| language.to_uppercase()),
        (!codec.is_empty()).then(|| codec.to_uppercase()),
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
    use super::{audio_track_label, non_negative_u32};

    #[test]
    fn rejects_negative_media_dimensions() {
        assert_eq!(non_negative_u32(-1), 0);
        assert_eq!(non_negative_u32(1_920), 1_920);
    }

    #[test]
    fn labels_audio_tracks_with_useful_fallbacks() {
        assert_eq!(
            audio_track_label(2, " Commentary ", "eng", "aac"),
            "Commentary  ·  ENG  ·  AAC"
        );
        assert_eq!(audio_track_label(1, "", "rus", ""), "Дорожка 1  ·  RUS");
        assert_eq!(audio_track_label(3, "", "", ""), "Дорожка 3");
    }
}
