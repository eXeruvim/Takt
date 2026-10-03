use std::path::Path;

pub(crate) fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

pub(crate) fn format_remaining_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_owned();
    }
    let rounded = seconds.round() as u64;
    if rounded < 60 {
        format!("{rounded} сек")
    } else {
        let minutes = rounded / 60;
        let seconds = rounded % 60;
        format!("{minutes} мин {seconds:02} сек")
    }
}

pub(crate) fn format_container(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "—".to_owned();
    }

    let lower = value.to_ascii_lowercase();
    if lower.contains("matroska") || lower.contains("webm") {
        "Matroska / WebM".to_owned()
    } else if lower.contains("mp4") || lower.contains("mov") {
        "MPEG-4 / MOV".to_owned()
    } else {
        value.to_uppercase()
    }
}

pub(crate) fn format_codec(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "—".to_owned()
    } else {
        value.to_uppercase()
    }
}

pub(crate) fn format_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    if bytes == 0 {
        "—".to_owned()
    } else if bytes as f64 >= GIB {
        format!("{:.2} ГБ", bytes as f64 / GIB).replace('.', ",")
    } else if bytes as f64 >= MIB {
        format!("{:.1} МБ", bytes as f64 / MIB).replace('.', ",")
    } else if bytes as f64 >= KIB {
        format!("{:.0} КБ", bytes as f64 / KIB)
    } else {
        format!("{bytes} Б")
    }
}

pub(crate) fn media_name_without_extension(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

pub(crate) fn format_precise_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    if bytes as f64 >= GIB {
        format!("{:.2} ГБ", bytes as f64 / GIB).replace('.', ",")
    } else if bytes as f64 >= MIB {
        format!("{:.2} МБ", bytes as f64 / MIB).replace('.', ",")
    } else if bytes as f64 >= KIB {
        format!("{:.1} КБ", bytes as f64 / KIB).replace('.', ",")
    } else {
        format!("{bytes} Б")
    }
}

pub(crate) fn format_size_change(source_bytes: u64, output_bytes: u64) -> String {
    let output = format_precise_file_size(output_bytes);
    if source_bytes == 0 {
        return format!("Результат: {output}");
    }
    let difference = output_bytes as f64 / source_bytes as f64 - 1.0;
    let percentage = (difference.abs() * 100.0).round() as u64;
    if difference < -0.005 {
        format!("Результат: {output} · на {percentage}% меньше")
    } else if difference > 0.005 {
        format!("Результат: {output} · на {percentage}% больше")
    } else {
        format!("Результат: {output} · почти без изменения")
    }
}

pub(crate) fn format_bitrate(bits_per_second: f64) -> String {
    if !bits_per_second.is_finite() || bits_per_second <= 0.0 {
        "—".to_owned()
    } else if bits_per_second >= 1_000_000.0 {
        format!("{:.2} Мбит/с", bits_per_second / 1_000_000.0).replace('.', ",")
    } else {
        format!("{:.0} кбит/с", bits_per_second / 1_000.0)
    }
}

pub(crate) fn format_track_summary(video: u32, audio: u32, subtitles: u32) -> String {
    format!("Видео {video}  ·  Аудио {audio}  ·  Субтитры {subtitles}")
}

pub(crate) fn is_audio_only_media(
    video_track_count: u32,
    video_codec: &str,
    audio_codec: &str,
) -> bool {
    video_track_count == 0 && video_codec.is_empty() && !audio_codec.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        format_bitrate, format_container, format_file_size, format_remaining_time,
        format_size_change, format_time, format_track_summary, is_audio_only_media,
        media_name_without_extension,
    };

    #[test]
    fn formats_short_and_long_durations() {
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(65.0), "01:05");
        assert_eq!(format_time(3_661.0), "01:01:01");
        assert_eq!(format_time(-10.0), "00:00");
    }

    #[test]
    fn formats_remaining_time_and_rejects_unknown_values() {
        assert_eq!(format_remaining_time(17.0), "17 сек");
        assert_eq!(format_remaining_time(125.0), "2 мин 05 сек");
        assert_eq!(format_remaining_time(-1.0), "—");
        assert_eq!(format_remaining_time(f64::NAN), "—");
    }

    #[test]
    fn hides_extension_only_in_the_audio_canvas_title() {
        assert_eq!(
            media_name_without_extension("Artist - Track.name.mp3"),
            "Artist - Track.name"
        );
        assert_eq!(
            media_name_without_extension("Без расширения"),
            "Без расширения"
        );
    }

    #[test]
    fn formats_local_media_properties_for_the_inspector() {
        assert_eq!(format_container("mov,mp4,m4a,3gp"), "MPEG-4 / MOV");
        assert_eq!(format_container("matroska,webm"), "Matroska / WebM");
        assert_eq!(format_file_size(1_048_576), "1,0 МБ");
        assert_eq!(format_file_size(1_073_741_824), "1,00 ГБ");
        assert_eq!(format_bitrate(800_000.0), "800 кбит/с");
        assert_eq!(format_bitrate(2_500_000.0), "2,50 Мбит/с");
        assert_eq!(
            format_track_summary(1, 2, 3),
            "Видео 1  ·  Аудио 2  ·  Субтитры 3"
        );
    }

    #[test]
    fn reports_actual_conversion_size_change() {
        assert_eq!(
            format_size_change(100 * 1024 * 1024, 60 * 1024 * 1024),
            "Результат: 60,00 МБ · на 40% меньше"
        );
        assert_eq!(
            format_size_change(100 * 1024 * 1024, 125 * 1024 * 1024),
            "Результат: 125,00 МБ · на 25% больше"
        );
    }

    #[test]
    fn detects_audio_only_media_from_actual_tracks_not_file_extension() {
        assert!(is_audio_only_media(0, "", "MP3 (MPEG audio layer 3)"));
        assert!(!is_audio_only_media(1, "", "AAC"));
        assert!(!is_audio_only_media(1, "yuv420p", "AAC"));
        assert!(!is_audio_only_media(0, "", ""));
    }
}
