use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

#[cfg(target_os = "windows")]
use std::{
    iter,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
};

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0},
    System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject},
};

const DIAGNOSTICS_ENV: &str = "TAKT_BENCHMARK_DIAGNOSTICS_PATH";
const MEASUREMENT_GATE_ENV: &str = "TAKT_BENCHMARK_RENDER_GATE";
const UI_LATENCY_ENV: &str = "TAKT_UI_LATENCY_DIAGNOSTICS";
const UI_CYCLE_COUNT_ENV: &str = "TAKT_BENCHMARK_UI_CYCLES";
const FAULT_ENV: &str = "TAKT_BENCHMARK_FAULT";
const MAX_TIMING_SAMPLES: usize = 300_000;
const MAX_UI_CYCLES: u64 = 10_000;

static COMPLETED_UI_CYCLES: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "benchmark-tools")]
fn fault_matches(value: Option<&std::ffi::OsStr>, expected: &str) -> bool {
    value.is_some_and(|value| value == std::ffi::OsStr::new(expected))
}

/// Returns whether a profiling-only failure scenario was explicitly requested.
///
/// The production binary compiles this to an unconditional `false`, so setting
/// the environment variable cannot alter normal playback behavior.
pub fn fault_requested(expected: &str) -> bool {
    #[cfg(feature = "benchmark-tools")]
    {
        static FAULT: OnceLock<Option<std::ffi::OsString>> = OnceLock::new();
        fault_matches(
            FAULT.get_or_init(|| std::env::var_os(FAULT_ENV)).as_deref(),
            expected,
        )
    }

    #[cfg(not(feature = "benchmark-tools"))]
    {
        let _ = (FAULT_ENV, expected);
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderSurface {
    Empty,
    Video,
    Audio,
}

impl RenderSurface {
    const COUNT: usize = 3;

    fn index(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Video => 1,
            Self::Audio => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiSurface {
    Playlist,
    Info,
    Settings,
    CommandPalette,
}

impl UiSurface {
    const COUNT: usize = 4;

    fn index(self) -> usize {
        match self {
            Self::Playlist => 0,
            Self::Info => 1,
            Self::Settings => 2,
            Self::CommandPalette => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Playlist => "Очередь",
            Self::Info => "Инфо",
            Self::Settings => "Настройки",
            Self::CommandPalette => "Горячие клавиши",
        }
    }
}

#[derive(Clone, Copy)]
struct PendingUiLatency {
    surface: UiSurface,
    started: Instant,
    rendering_started: Option<Instant>,
    opening: u64,
}

struct UiLatencyState {
    pending: Option<PendingUiLatency>,
    openings: [u64; UiSurface::COUNT],
}

impl Default for UiLatencyState {
    fn default() -> Self {
        Self {
            pending: None,
            openings: [0; UiSurface::COUNT],
        }
    }
}

impl UiLatencyState {
    fn begin(&mut self, surface: UiSurface, now: Instant) {
        let opening = self.openings[surface.index()].saturating_add(1);
        self.openings[surface.index()] = opening;
        self.pending = Some(PendingUiLatency {
            surface,
            started: now,
            rendering_started: None,
            opening,
        });
    }

    fn begin_rendering(&mut self, now: Instant) {
        if let Some(pending) = self.pending.as_mut()
            && pending.rendering_started.is_none()
        {
            pending.rendering_started = Some(now);
        }
    }

    fn finish(&mut self, now: Instant) -> Option<(UiSurface, u64, f64, f64, f64)> {
        let pending = self.pending.take()?;
        let rendering_started = pending.rendering_started.unwrap_or(now);
        Some((
            pending.surface,
            pending.opening,
            now.saturating_duration_since(pending.started).as_secs_f64() * 1_000.0,
            rendering_started
                .saturating_duration_since(pending.started)
                .as_secs_f64()
                * 1_000.0,
            now.saturating_duration_since(rendering_started)
                .as_secs_f64()
                * 1_000.0,
        ))
    }
}

fn ui_latency_state() -> Option<&'static Mutex<UiLatencyState>> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static STATE: OnceLock<Mutex<UiLatencyState>> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os(UI_LATENCY_ENV).is_some()) {
        return None;
    }
    Some(STATE.get_or_init(|| Mutex::new(UiLatencyState::default())))
}

pub fn begin_ui_latency(surface: UiSurface) {
    if let Some(state) = ui_latency_state()
        && let Ok(mut state) = state.lock()
    {
        state.begin(surface, Instant::now());
    }
}

pub fn begin_ui_latency_render() {
    if let Some(state) = ui_latency_state()
        && let Ok(mut state) = state.lock()
    {
        state.begin_rendering(Instant::now());
    }
}

pub fn finish_ui_latency_frame() {
    let Some(state) = ui_latency_state() else {
        return;
    };
    let sample = state
        .lock()
        .ok()
        .and_then(|mut state| state.finish(Instant::now()));
    if let Some((surface, opening, elapsed_ms, before_render_ms, render_ms)) = sample {
        eprintln!(
            "UI: {} — кадр готов за {elapsed_ms:.1} мс (до рендера {before_render_ms:.1} мс, рендер {render_ms:.1} мс; открытие {opening})",
            surface.label()
        );
    }
}

#[derive(Clone, Copy)]
struct PendingSceneFrame {
    started: Instant,
    surface: RenderSurface,
    mpv_render_ms: f64,
}

struct RenderTimingState {
    collection_active: bool,
    last_render_started: Option<Instant>,
    frame_intervals_ms: Vec<f64>,
    render_durations_ms: Vec<f64>,
    discarded_intervals: u64,
    discarded_durations: u64,
    pending_scene_frame: Option<PendingSceneFrame>,
    last_scene_started: [Option<Instant>; RenderSurface::COUNT],
    scene_intervals_ms: [Vec<f64>; RenderSurface::COUNT],
    scene_durations_ms: [Vec<f64>; RenderSurface::COUNT],
    slint_scene_durations_ms: [Vec<f64>; RenderSurface::COUNT],
    discarded_scene_intervals: [u64; RenderSurface::COUNT],
    discarded_scene_durations: [u64; RenderSurface::COUNT],
    discarded_slint_scene_durations: [u64; RenderSurface::COUNT],
}

impl Default for RenderTimingState {
    fn default() -> Self {
        Self {
            collection_active: false,
            last_render_started: None,
            frame_intervals_ms: Vec::new(),
            render_durations_ms: Vec::new(),
            discarded_intervals: 0,
            discarded_durations: 0,
            pending_scene_frame: None,
            last_scene_started: [None; RenderSurface::COUNT],
            scene_intervals_ms: std::array::from_fn(|_| Vec::new()),
            scene_durations_ms: std::array::from_fn(|_| Vec::new()),
            slint_scene_durations_ms: std::array::from_fn(|_| Vec::new()),
            discarded_scene_intervals: [0; RenderSurface::COUNT],
            discarded_scene_durations: [0; RenderSurface::COUNT],
            discarded_slint_scene_durations: [0; RenderSurface::COUNT],
        }
    }
}

impl RenderTimingState {
    fn update_collection_gate(&mut self, gate_open: bool) -> bool {
        if !gate_open {
            self.collection_active = false;
            self.last_render_started = None;
            self.pending_scene_frame = None;
            self.last_scene_started = [None; RenderSurface::COUNT];
            return false;
        }
        if !self.collection_active {
            *self = Self {
                collection_active: true,
                ..Self::default()
            };
        }
        true
    }
}

#[cfg(target_os = "windows")]
struct MeasurementGate {
    configured: bool,
    handle: Option<OwnedHandle>,
}

#[cfg(target_os = "windows")]
impl MeasurementGate {
    fn from_environment() -> Self {
        let Some(name) = std::env::var_os(MEASUREMENT_GATE_ENV) else {
            return Self {
                configured: false,
                handle: None,
            };
        };
        let wide = name.encode_wide().chain(iter::once(0)).collect::<Vec<_>>();
        // SAFETY: `wide` is a valid, null-terminated event name for the duration
        // of the call. The returned handle is immediately given to OwnedHandle.
        let handle = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide.as_ptr()) };
        let handle = if handle.is_null() {
            eprintln!("Не удалось открыть окно frame-time измерения");
            None
        } else {
            // SAFETY: OpenEventW returned a new owned handle which is non-null.
            Some(unsafe { OwnedHandle::from_raw_handle(handle) })
        };
        Self {
            configured: true,
            handle,
        }
    }

    fn is_open(&self) -> bool {
        if !self.configured {
            return true;
        }
        let Some(handle) = self.handle.as_ref() else {
            return false;
        };
        // SAFETY: OwnedHandle keeps the event handle valid for the call.
        unsafe { WaitForSingleObject(handle.as_raw_handle() as HANDLE, 0) == WAIT_OBJECT_0 }
    }
}

#[cfg(target_os = "windows")]
fn measurement_gate() -> &'static MeasurementGate {
    static GATE: OnceLock<MeasurementGate> = OnceLock::new();
    GATE.get_or_init(MeasurementGate::from_environment)
}

#[cfg(target_os = "windows")]
fn measurement_gate_open() -> bool {
    measurement_gate().is_open()
}

pub fn measurement_window_open() -> bool {
    measurement_gate_open()
}

fn parse_ui_cycle_count(value: Option<std::ffi::OsString>) -> Option<u64> {
    let value = value?.to_str()?.parse::<u64>().ok()?;
    (1..=MAX_UI_CYCLES).contains(&value).then_some(value)
}

pub fn requested_ui_cycles() -> Option<u64> {
    static REQUESTED: OnceLock<Option<u64>> = OnceLock::new();
    *REQUESTED.get_or_init(|| parse_ui_cycle_count(std::env::var_os(UI_CYCLE_COUNT_ENV)))
}

pub fn record_ui_cycle_completed() -> u64 {
    COMPLETED_UI_CYCLES.fetch_add(1, Ordering::Relaxed) + 1
}

fn ui_cycle_summary() -> serde_json::Value {
    let Some(requested) = requested_ui_cycles() else {
        return serde_json::Value::Null;
    };
    let completed = COMPLETED_UI_CYCLES.load(Ordering::Relaxed);
    serde_json::json!({
        "requested": requested,
        "completed": completed,
        "complete": completed == requested,
        "sequence": "playlist -> settings -> closed",
    })
}

#[cfg(not(target_os = "windows"))]
fn measurement_gate_open() -> bool {
    true
}

fn measurement_gate_configured() -> bool {
    #[cfg(target_os = "windows")]
    {
        measurement_gate().configured
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

fn measurement_gate_available() -> bool {
    #[cfg(target_os = "windows")]
    {
        let gate = measurement_gate();
        gate.configured && gate.handle.is_some()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[derive(Debug)]
pub struct TimingStats {
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

fn state() -> Option<&'static Mutex<RenderTimingState>> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static STATE: OnceLock<Mutex<RenderTimingState>> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os(DIAGNOSTICS_ENV).is_some()) {
        return None;
    }
    Some(STATE.get_or_init(|| Mutex::new(RenderTimingState::default())))
}

fn push_bounded(values: &mut Vec<f64>, discarded: &mut u64, value: f64) {
    if values.len() < MAX_TIMING_SAMPLES {
        values.push(value);
    } else {
        *discarded = discarded.saturating_add(1);
    }
}

pub fn reset_render_timing() {
    if let Some(state) = state()
        && let Ok(mut state) = state.lock()
    {
        *state = RenderTimingState::default();
    }
}

pub fn begin_scene_frame(surface: RenderSurface) {
    let Some(state) = state() else {
        return;
    };
    let gate_open = measurement_gate_open();
    let now = Instant::now();
    if let Ok(mut state) = state.lock() {
        if !state.update_collection_gate(gate_open) {
            return;
        }
        let index = surface.index();
        if let Some(previous) = state.last_scene_started[index] {
            let interval_ms = now.duration_since(previous).as_secs_f64() * 1_000.0;
            let RenderTimingState {
                scene_intervals_ms,
                discarded_scene_intervals,
                ..
            } = &mut *state;
            push_bounded(
                &mut scene_intervals_ms[index],
                &mut discarded_scene_intervals[index],
                interval_ms,
            );
        }
        state.last_scene_started[index] = Some(now);
        state.pending_scene_frame = Some(PendingSceneFrame {
            started: now,
            surface,
            mpv_render_ms: 0.0,
        });
    }
}

pub fn finish_scene_frame() {
    let Some(state) = state() else {
        return;
    };
    if let Ok(mut state) = state.lock()
        && let Some(frame) = state.pending_scene_frame.take()
    {
        let duration_ms = frame.started.elapsed().as_secs_f64() * 1_000.0;
        let slint_duration_ms = (duration_ms - frame.mpv_render_ms).max(0.0);
        let index = frame.surface.index();
        let RenderTimingState {
            scene_durations_ms,
            slint_scene_durations_ms,
            discarded_scene_durations,
            discarded_slint_scene_durations,
            ..
        } = &mut *state;
        push_bounded(
            &mut scene_durations_ms[index],
            &mut discarded_scene_durations[index],
            duration_ms,
        );
        push_bounded(
            &mut slint_scene_durations_ms[index],
            &mut discarded_slint_scene_durations[index],
            slint_duration_ms,
        );
    }
}

pub fn begin_render() -> Option<Instant> {
    let state = state()?;
    let gate_open = measurement_gate_open();
    let now = Instant::now();
    if let Ok(mut state) = state.lock() {
        if !state.update_collection_gate(gate_open) {
            return None;
        }
        if let Some(previous) = state.last_render_started {
            let interval_ms = now.duration_since(previous).as_secs_f64() * 1000.0;
            let RenderTimingState {
                frame_intervals_ms,
                discarded_intervals,
                ..
            } = &mut *state;
            push_bounded(frame_intervals_ms, discarded_intervals, interval_ms);
        }
        state.last_render_started = Some(now);
    }
    Some(now)
}

pub fn finish_render(started: Option<Instant>) {
    let (Some(state), Some(started)) = (state(), started) else {
        return;
    };
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Ok(mut state) = state.lock() {
        if let Some(frame) = state.pending_scene_frame.as_mut() {
            frame.mpv_render_ms += duration_ms;
        }
        let RenderTimingState {
            render_durations_ms,
            discarded_durations,
            ..
        } = &mut *state;
        push_bounded(render_durations_ms, discarded_durations, duration_ms);
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((percentile * sorted.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn summarize(values: &[f64]) -> Option<TimingStats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let round = |value: f64| (value * 1000.0).round() / 1000.0;
    Some(TimingStats {
        min: round(sorted[0]),
        p50: round(percentile(&sorted, 0.50)),
        p95: round(percentile(&sorted, 0.95)),
        p99: round(percentile(&sorted, 0.99)),
        max: round(sorted[sorted.len() - 1]),
        mean: round(sorted.iter().sum::<f64>() / sorted.len() as f64),
    })
}

fn stats_json(stats: Option<TimingStats>) -> serde_json::Value {
    let Some(stats) = stats else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "min": stats.min,
        "p50": stats.p50,
        "p95": stats.p95,
        "p99": stats.p99,
        "max": stats.max,
        "mean": stats.mean,
    })
}

fn surface_timing_json(state: &RenderTimingState, surface: RenderSurface) -> serde_json::Value {
    let index = surface.index();
    serde_json::json!({
        "rendered_frames": state.scene_durations_ms[index].len() as u64
            + state.discarded_scene_durations[index],
        "frame_interval_samples": state.scene_intervals_ms[index].len(),
        "scene_duration_samples": state.scene_durations_ms[index].len(),
        "discarded_intervals": state.discarded_scene_intervals[index],
        "discarded_scene_durations": state.discarded_scene_durations[index],
        "discarded_slint_scene_durations": state.discarded_slint_scene_durations[index],
        "frame_interval_ms": stats_json(summarize(&state.scene_intervals_ms[index])),
        "notifier_to_scene_complete_ms": stats_json(summarize(&state.scene_durations_ms[index])),
        "slint_scene_and_notifier_overhead_ms": stats_json(summarize(
            &state.slint_scene_durations_ms[index]
        )),
    })
}

pub fn render_timing_summary() -> Option<serde_json::Value> {
    let state = state()?;
    let state = state.lock().ok()?;
    let mut scene_by_surface = serde_json::Map::new();
    for surface in [
        RenderSurface::Empty,
        RenderSurface::Video,
        RenderSurface::Audio,
    ] {
        scene_by_surface.insert(
            surface.label().to_owned(),
            surface_timing_json(&state, surface),
        );
    }
    Some(serde_json::json!({
        "rendered_frames": state.render_durations_ms.len() as u64
            + state.discarded_durations,
        "frame_interval_samples": state.frame_intervals_ms.len(),
        "render_duration_samples": state.render_durations_ms.len(),
        "discarded_intervals": state.discarded_intervals,
        "discarded_durations": state.discarded_durations,
        "measurement_gate_configured": measurement_gate_configured(),
        "measurement_gate_available": measurement_gate_available(),
        "ui_cycles": ui_cycle_summary(),
        "scope": if measurement_gate_configured() {
            "benchmark measurement window"
        } else {
            "current media since load, including setup and warmup"
        },
        "frame_interval_ms": stats_json(summarize(&state.frame_intervals_ms)),
        "mpv_render_duration_ms": stats_json(summarize(&state.render_durations_ms)),
        "scene_by_surface": scene_by_surface,
        "metric_note": "mpv intervals cover integrated video renders. Scene intervals and durations are split by the surface visible at BeforeRendering. notifier_to_scene_complete_ms covers Slint BeforeRendering through AfterRendering; subtracting nested mpv_render produces an estimate of Slint scene rendering plus Takt notifier overhead. None of these metrics include buffer presentation, DWM latency, or GPU completion.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_latency_separates_cold_and_warm_openings() {
        let started = Instant::now();
        let mut state = UiLatencyState::default();

        state.begin(UiSurface::CommandPalette, started);
        state.begin_rendering(started + std::time::Duration::from_millis(75));
        let (surface, opening, elapsed_ms, before_render_ms, render_ms) = state
            .finish(started + std::time::Duration::from_millis(125))
            .expect("first rendered frame must finish the pending sample");
        assert_eq!(surface, UiSurface::CommandPalette);
        assert_eq!(opening, 1);
        assert_eq!(elapsed_ms, 125.0);
        assert_eq!(before_render_ms, 75.0);
        assert_eq!(render_ms, 50.0);

        state.begin(UiSurface::CommandPalette, started);
        let (_, opening, _, _, _) = state
            .finish(started + std::time::Duration::from_millis(8))
            .expect("second opening must produce its own sample");
        assert_eq!(opening, 2);
        assert!(state.finish(started).is_none());
    }

    #[test]
    fn percentile_uses_nearest_rank_and_rounds_milliseconds() {
        let stats = summarize(&[4.0004, 1.0, 3.0, 2.0]).expect("stats");
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.p50, 2.0);
        assert_eq!(stats.p95, 4.0);
        assert_eq!(stats.p99, 4.0);
        assert_eq!(stats.max, 4.0);
        assert_eq!(stats.mean, 2.5);
    }

    #[test]
    fn empty_timing_has_no_fake_zero_stats() {
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn scene_summary_keeps_audio_and_video_samples_separate() {
        let mut state = RenderTimingState::default();
        let audio = RenderSurface::Audio.index();
        let video = RenderSurface::Video.index();
        state.scene_intervals_ms[audio] = vec![33.0, 34.0];
        state.scene_durations_ms[audio] = vec![2.0, 4.0];
        state.slint_scene_durations_ms[audio] = vec![2.0, 4.0];
        state.scene_intervals_ms[video] = vec![16.0];
        state.scene_durations_ms[video] = vec![5.0];
        state.slint_scene_durations_ms[video] = vec![1.5];

        let audio_summary = surface_timing_json(&state, RenderSurface::Audio);
        let video_summary = surface_timing_json(&state, RenderSurface::Video);

        assert_eq!(audio_summary["rendered_frames"], 2);
        assert_eq!(audio_summary["frame_interval_ms"]["p50"], 33.0);
        assert_eq!(audio_summary["notifier_to_scene_complete_ms"]["p95"], 4.0);
        assert_eq!(video_summary["rendered_frames"], 1);
        assert_eq!(
            video_summary["slint_scene_and_notifier_overhead_ms"]["p50"],
            1.5
        );
    }

    #[test]
    fn measurement_gate_preserves_finished_samples_then_resets_on_next_start() {
        let mut state = RenderTimingState {
            collection_active: true,
            last_render_started: Some(Instant::now()),
            frame_intervals_ms: vec![33.0],
            render_durations_ms: vec![2.0],
            pending_scene_frame: Some(PendingSceneFrame {
                started: Instant::now(),
                surface: RenderSurface::Video,
                mpv_render_ms: 2.0,
            }),
            last_scene_started: [Some(Instant::now()); RenderSurface::COUNT],
            ..RenderTimingState::default()
        };

        assert!(!state.update_collection_gate(false));
        assert!(!state.collection_active);
        assert!(state.last_render_started.is_none());
        assert!(state.pending_scene_frame.is_none());
        assert_eq!(state.last_scene_started, [None; RenderSurface::COUNT]);
        assert_eq!(state.frame_intervals_ms, [33.0]);
        assert_eq!(state.render_durations_ms, [2.0]);

        assert!(state.update_collection_gate(true));
        assert!(state.collection_active);
        assert!(state.frame_intervals_ms.is_empty());
        assert!(state.render_durations_ms.is_empty());
    }

    #[test]
    fn ui_cycle_count_is_bounded_and_strict() {
        assert_eq!(parse_ui_cycle_count(Some("100".into())), Some(100));
        assert_eq!(parse_ui_cycle_count(Some("10000".into())), Some(10_000));
        assert_eq!(parse_ui_cycle_count(Some("0".into())), None);
        assert_eq!(parse_ui_cycle_count(Some("10001".into())), None);
        assert_eq!(parse_ui_cycle_count(Some("not-a-number".into())), None);
    }

    #[cfg(feature = "benchmark-tools")]
    #[test]
    fn profiling_fault_requires_an_exact_scenario_name() {
        assert!(fault_matches(Some("mpv-init".as_ref()), "mpv-init"));
        assert!(!fault_matches(Some("renderer-init".as_ref()), "mpv-init"));
        assert!(!fault_matches(None, "mpv-init"));
    }
}
