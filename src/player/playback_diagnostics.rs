//! Optional mpv counter collection used by reproducible profiling runs.

use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context;
use libmpv::{
    Format,
    events::{EventContext, PropertyData},
};

const BENCHMARK_DIAGNOSTICS_ENV: &str = "OXIDE_BENCHMARK_DIAGNOSTICS_PATH";

#[derive(Default)]
struct IntervalStats {
    active_since: Option<Instant>,
    count: u64,
    completed: Duration,
    longest: Duration,
}

impl IntervalStats {
    fn update(&mut self, active: bool) {
        match (active, self.active_since) {
            (true, None) => {
                self.count += 1;
                self.active_since = Some(Instant::now());
            }
            (false, Some(started)) => {
                let elapsed = started.elapsed();
                self.completed += elapsed;
                self.longest = self.longest.max(elapsed);
                self.active_since = None;
            }
            _ => {}
        }
    }

    fn durations(&self) -> (Duration, Duration) {
        let pending = self
            .active_since
            .map_or(Duration::ZERO, |started| started.elapsed());
        (self.completed + pending, self.longest.max(pending))
    }
}

#[derive(Default)]
pub(super) struct PlaybackDiagnostics {
    output_path: Option<PathBuf>,
    decoder: Option<String>,
    video_sync: Option<String>,
    frame_drop_count: Option<i64>,
    decoder_frame_drop_count: Option<i64>,
    mistimed_frame_count: Option<i64>,
    vo_delayed_frame_count: Option<i64>,
    container_fps: Option<f64>,
    estimated_vf_fps: Option<f64>,
    display_fps: Option<f64>,
    estimated_display_fps: Option<f64>,
    vsync_ratio: Option<f64>,
    vsync_jitter: Option<f64>,
    avsync_seconds: Option<f64>,
    total_avsync_change_seconds: Option<f64>,
    time_pos_seconds: Option<f64>,
    demuxer_cache_duration_seconds: Option<f64>,
    demuxer_cache_duration_min_seconds: Option<f64>,
    demuxer_cache_duration_max_seconds: Option<f64>,
    cache_read_rate_bytes_per_second: Option<i64>,
    cache_read_rate_max_bytes_per_second: Option<i64>,
    cache_pauses: IntervalStats,
    seeking: IntervalStats,
}

impl PlaybackDiagnostics {
    pub(super) fn from_environment() -> Self {
        Self {
            output_path: diagnostics_path_from(std::env::var_os(BENCHMARK_DIAGNOSTICS_ENV)),
            ..Self::default()
        }
    }

    pub(super) fn observe_properties(&self, events: &mut EventContext<'_>) {
        if self.output_path.is_none() {
            return;
        }
        for (id, name, format) in [
            (100, "hwdec-current", Format::String),
            (101, "video-sync", Format::String),
            (102, "frame-drop-count", Format::Int64),
            (103, "decoder-frame-drop-count", Format::Int64),
            (104, "mistimed-frame-count", Format::Int64),
            (105, "vo-delayed-frame-count", Format::Int64),
            (106, "container-fps", Format::Double),
            (107, "estimated-vf-fps", Format::Double),
            (108, "display-fps", Format::Double),
            (109, "estimated-display-fps", Format::Double),
            (110, "vsync-ratio", Format::Double),
            (111, "vsync-jitter", Format::Double),
            (112, "avsync", Format::Double),
            (113, "total-avsync-change", Format::Double),
            (114, "demuxer-cache-duration", Format::Double),
            (115, "cache-speed", Format::Int64),
            (116, "paused-for-cache", Format::Flag),
            (117, "seeking", Format::Flag),
        ] {
            if let Err(error) = events.observe_property(name, format, id) {
                eprintln!("Не удалось наблюдать диагностическое свойство {name}: {error:?}");
            }
        }
    }

    pub(super) fn reset_for_new_media(&mut self) {
        let output_path = self.output_path.take();
        let video_sync = self.video_sync.take();
        let display_fps = self.display_fps.take();
        *self = Self {
            output_path,
            video_sync,
            display_fps,
            ..Self::default()
        };
        crate::benchmark_diagnostics::reset_render_timing();
    }

    pub(super) fn update(&mut self, name: &str, change: &PropertyData<'_>) {
        match (name, change) {
            ("hwdec-current", PropertyData::Str(value)) => {
                self.decoder = Some((*value).to_owned());
            }
            ("video-sync", PropertyData::Str(value)) => {
                self.video_sync = Some((*value).to_owned());
            }
            ("frame-drop-count", PropertyData::Int64(value)) => {
                self.frame_drop_count = Some(*value);
            }
            ("decoder-frame-drop-count", PropertyData::Int64(value)) => {
                self.decoder_frame_drop_count = Some(*value);
            }
            ("mistimed-frame-count", PropertyData::Int64(value)) => {
                self.mistimed_frame_count = Some(*value);
            }
            ("vo-delayed-frame-count", PropertyData::Int64(value)) => {
                self.vo_delayed_frame_count = Some(*value);
            }
            ("container-fps", PropertyData::Double(value)) => {
                self.container_fps = Some(*value);
            }
            ("estimated-vf-fps", PropertyData::Double(value)) => {
                self.estimated_vf_fps = Some(*value);
            }
            ("display-fps", PropertyData::Double(value)) => {
                self.display_fps = Some(*value);
            }
            ("estimated-display-fps", PropertyData::Double(value)) => {
                self.estimated_display_fps = Some(*value);
            }
            ("vsync-ratio", PropertyData::Double(value)) => {
                self.vsync_ratio = Some(*value);
            }
            ("vsync-jitter", PropertyData::Double(value)) => {
                self.vsync_jitter = Some(*value);
            }
            ("avsync", PropertyData::Double(value)) => {
                self.avsync_seconds = Some(*value);
            }
            ("total-avsync-change", PropertyData::Double(value)) => {
                self.total_avsync_change_seconds = Some(*value);
            }
            ("time-pos", PropertyData::Double(value)) => {
                self.time_pos_seconds = Some(*value);
            }
            ("demuxer-cache-duration", PropertyData::Double(value))
                if value.is_finite() && *value >= 0.0 =>
            {
                self.demuxer_cache_duration_seconds = Some(*value);
                self.demuxer_cache_duration_min_seconds = Some(
                    self.demuxer_cache_duration_min_seconds
                        .map_or(*value, |current| current.min(*value)),
                );
                self.demuxer_cache_duration_max_seconds = Some(
                    self.demuxer_cache_duration_max_seconds
                        .map_or(*value, |current| current.max(*value)),
                );
            }
            ("cache-speed", PropertyData::Int64(value)) if *value >= 0 => {
                self.cache_read_rate_bytes_per_second = Some(*value);
                self.cache_read_rate_max_bytes_per_second = Some(
                    self.cache_read_rate_max_bytes_per_second
                        .map_or(*value, |current| current.max(*value)),
                );
            }
            ("paused-for-cache", PropertyData::Flag(value)) => {
                self.cache_pauses.update(*value);
            }
            ("seeking", PropertyData::Flag(value)) => {
                self.seeking.update(*value);
            }
            _ => {}
        }
    }

    pub(super) fn write_report(&self, media_available: bool) -> anyhow::Result<()> {
        let Some(path) = self.output_path.as_deref() else {
            return Ok(());
        };
        write_report(self, path, media_available)
    }
}

fn diagnostics_path_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    if !path.is_absolute() || path.extension().and_then(|value| value.to_str()) != Some("json") {
        eprintln!("Путь диагностического отчёта проигнорирован: требуется абсолютный JSON-путь");
        return None;
    }
    Some(path)
}

fn write_report(
    diagnostics: &PlaybackDiagnostics,
    path: &Path,
    media_available: bool,
) -> anyhow::Result<()> {
    let (cache_pause_total, cache_pause_longest) = diagnostics.cache_pauses.durations();
    let (seeking_total, seeking_longest) = diagnostics.seeking.durations();
    let report = serde_json::json!({
        "schema": 1,
        "kind": "oxide-mpv-playback-diagnostics",
        "scope": "mpv counters cover current media since load; render_timing declares its own scope",
        "media_available": media_available,
        "decoder": diagnostics.decoder,
        "video_sync": diagnostics.video_sync,
        "frame_drop_count": diagnostics.frame_drop_count,
        "decoder_frame_drop_count": diagnostics.decoder_frame_drop_count,
        "mistimed_frame_count": diagnostics.mistimed_frame_count,
        "vo_delayed_frame_count": diagnostics.vo_delayed_frame_count,
        "container_fps": diagnostics.container_fps,
        "estimated_vf_fps": diagnostics.estimated_vf_fps,
        "display_fps": diagnostics.display_fps,
        "estimated_display_fps": diagnostics.estimated_display_fps,
        "vsync_ratio": diagnostics.vsync_ratio,
        "vsync_jitter": diagnostics.vsync_jitter,
        "avsync_seconds": diagnostics.avsync_seconds,
        "total_avsync_change_seconds": diagnostics.total_avsync_change_seconds,
        "time_pos_seconds": diagnostics.time_pos_seconds,
        "demuxer_cache": {
            "duration_seconds": diagnostics.demuxer_cache_duration_seconds,
            "duration_min_seconds": diagnostics.demuxer_cache_duration_min_seconds,
            "duration_max_seconds": diagnostics.demuxer_cache_duration_max_seconds,
            "read_rate_bytes_per_second": diagnostics.cache_read_rate_bytes_per_second,
            "read_rate_max_bytes_per_second": diagnostics.cache_read_rate_max_bytes_per_second,
            "pause_count": diagnostics.cache_pauses.count,
            "pause_total_ms": duration_ms(cache_pause_total),
            "pause_max_ms": duration_ms(cache_pause_longest),
        },
        "seeking": {
            "count": diagnostics.seeking.count,
            "total_ms": duration_ms(seeking_total),
            "max_ms": duration_ms(seeking_longest),
        },
        "render_timing": crate::benchmark_diagnostics::render_timing_summary(),
    });
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "Не удалось создать диагностический отчёт: {}",
                path.display()
            )
        })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &report)
        .context("Не удалось сериализовать диагностический отчёт")?;
    writer
        .flush()
        .context("Не удалось записать диагностический отчёт на диск")
}

fn duration_ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use std::time::{Duration, Instant};

    use super::{IntervalStats, diagnostics_path_from};

    #[test]
    fn diagnostics_path_requires_absolute_json_file() {
        assert!(diagnostics_path_from(None).is_none());
        assert!(diagnostics_path_from(Some(OsString::from("report.json"))).is_none());

        let wrong_extension = std::env::temp_dir().join("oxide-diagnostics.txt");
        assert!(diagnostics_path_from(Some(wrong_extension.into_os_string())).is_none());

        let expected = std::env::temp_dir().join("oxide-diagnostics.json");
        assert_eq!(
            diagnostics_path_from(Some(expected.clone().into_os_string())),
            Some(expected)
        );
    }

    #[test]
    fn interval_stats_count_transitions_once_and_include_active_time() {
        let mut stats = IntervalStats::default();
        stats.update(true);
        stats.update(true);
        stats.active_since = Some(Instant::now() - Duration::from_millis(25));

        let (active_total, active_longest) = stats.durations();
        assert_eq!(stats.count, 1);
        assert!(active_total >= Duration::from_millis(25));
        assert!(active_longest >= Duration::from_millis(25));

        stats.update(false);
        stats.update(false);
        let (finished_total, finished_longest) = stats.durations();
        assert_eq!(stats.count, 1);
        assert!(finished_total >= Duration::from_millis(25));
        assert_eq!(finished_total, finished_longest);
    }
}
