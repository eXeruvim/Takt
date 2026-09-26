use std::{
    sync::{Mutex, OnceLock},
    time::Instant,
};

const DIAGNOSTICS_ENV: &str = "OXIDE_BENCHMARK_DIAGNOSTICS_PATH";
const MAX_TIMING_SAMPLES: usize = 300_000;

#[derive(Default)]
struct RenderTimingState {
    last_render_started: Option<Instant>,
    frame_intervals_ms: Vec<f64>,
    render_durations_ms: Vec<f64>,
    discarded_intervals: u64,
    discarded_durations: u64,
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
    let now = Instant::now();
    if let Ok(mut state) = state.lock() {
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
        "frame_interval_ms": stats_json(summarize(&state.frame_intervals_ms)),
        "mpv_render_duration_ms": stats_json(summarize(&state.render_durations_ms)),
        "metric_note": "Intervals surround Slint's integrated OpenGL render callback; they are not DWM/display presentation latency.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
