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

fn non_negative_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::non_negative_u32;

    #[test]
    fn rejects_negative_media_dimensions() {
        assert_eq!(non_negative_u32(-1), 0);
        assert_eq!(non_negative_u32(1_920), 1_920);
    }
}
