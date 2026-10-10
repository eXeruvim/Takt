use std::{
    sync::{Mutex, OnceLock},
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

const DIAGNOSTICS_ENV: &str = "OXIDE_BENCHMARK_DIAGNOSTICS_PATH";
const MEASUREMENT_GATE_ENV: &str = "OXIDE_BENCHMARK_RENDER_GATE";
const UI_LATENCY_ENV: &str = "OXIDE_UI_LATENCY_DIAGNOSTICS";
const MAX_TIMING_SAMPLES: usize = 300_000;

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

#[derive(Default)]
struct RenderTimingState {
    collection_active: bool,
    last_render_started: Option<Instant>,
    frame_intervals_ms: Vec<f64>,
    render_durations_ms: Vec<f64>,
    discarded_intervals: u64,
    discarded_durations: u64,
}

impl RenderTimingState {
    fn update_collection_gate(&mut self, gate_open: bool) -> bool {
        if !gate_open {
            self.collection_active = false;
            self.last_render_started = None;
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

pub fn render_timing_summary() -> Option<serde_json::Value> {
    let state = state()?;
    let state = state.lock().ok()?;
    Some(serde_json::json!({
        "rendered_frames": state.render_durations_ms.len() as u64
            + state.discarded_durations,
        "frame_interval_samples": state.frame_intervals_ms.len(),
        "render_duration_samples": state.render_durations_ms.len(),
        "discarded_intervals": state.discarded_intervals,
        "discarded_durations": state.discarded_durations,
        "measurement_gate_configured": measurement_gate_configured(),
        "measurement_gate_available": measurement_gate_available(),
        "scope": if measurement_gate_configured() {
            "benchmark measurement window"
        } else {
            "current media since load, including setup and warmup"
        },
        "frame_interval_ms": stats_json(summarize(&state.frame_intervals_ms)),
        "mpv_render_duration_ms": stats_json(summarize(&state.render_durations_ms)),
        "metric_note": "Intervals surround Slint's integrated OpenGL render callback; they are not DWM/display presentation latency.",
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
    fn measurement_gate_preserves_finished_samples_then_resets_on_next_start() {
        let mut state = RenderTimingState {
            collection_active: true,
            last_render_started: Some(Instant::now()),
            frame_intervals_ms: vec![33.0],
            render_durations_ms: vec![2.0],
            ..RenderTimingState::default()
        };

        assert!(!state.update_collection_gate(false));
        assert!(!state.collection_active);
        assert!(state.last_render_started.is_none());
        assert_eq!(state.frame_intervals_ms, [33.0]);
        assert_eq!(state.render_durations_ms, [2.0]);

        assert!(state.update_collection_gate(true));
        assert!(state.collection_active);
        assert!(state.frame_intervals_ms.is_empty());
        assert!(state.render_durations_ms.is_empty());
    }
}
