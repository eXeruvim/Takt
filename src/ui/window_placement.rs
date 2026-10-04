use std::sync::{Arc, Mutex};

use slint::{
    ComponentHandle,
    winit_030::{
        WinitWindowAccessor,
        winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    },
};

use crate::settings::{AppSettings, WindowPlacementPreference};

use super::ui_logic::AppWindow;

#[derive(Clone, Copy, Debug, PartialEq)]
struct MonitorGeometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

impl MonitorGeometry {
    fn contains(self, x: i32, y: i32) -> bool {
        i64::from(x) >= i64::from(self.x)
            && i64::from(y) >= i64::from(self.y)
            && i64::from(x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(y) < i64::from(self.y) + i64::from(self.height)
    }
}

fn monitor_geometry(monitor: &slint::winit_030::winit::monitor::MonitorHandle) -> MonitorGeometry {
    let position = monitor.position();
    let size = monitor.size();
    MonitorGeometry {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        scale: monitor.scale_factor(),
    }
}

fn placement_intersection_area(
    placement: WindowPlacementPreference,
    monitor: MonitorGeometry,
) -> u64 {
    let scale = monitor.scale.max(0.1);
    let width = (f64::from(placement.width) * scale).round() as i64;
    let height = (f64::from(placement.height) * scale).round() as i64;
    let left = i64::from(placement.x).max(i64::from(monitor.x));
    let top = i64::from(placement.y).max(i64::from(monitor.y));
    let right =
        (i64::from(placement.x) + width).min(i64::from(monitor.x) + i64::from(monitor.width));
    let bottom =
        (i64::from(placement.y) + height).min(i64::from(monitor.y) + i64::from(monitor.height));

    u64::try_from((right - left).max(0))
        .unwrap_or_default()
        .saturating_mul(u64::try_from((bottom - top).max(0)).unwrap_or_default())
}

fn fit_window_placement(
    placement: WindowPlacementPreference,
    monitor: MonitorGeometry,
    preserve_position: bool,
) -> (PhysicalPosition<i32>, LogicalSize<f64>) {
    let scale = monitor.scale.max(0.1);
    let physical_width =
        ((f64::from(placement.width) * scale).round() as u32).min(monitor.width.max(1));
    let physical_height =
        ((f64::from(placement.height) * scale).round() as u32).min(monitor.height.max(1));
    let max_x = i64::from(monitor.x) + i64::from(monitor.width - physical_width);
    let max_y = i64::from(monitor.y) + i64::from(monitor.height - physical_height);
    let x = if preserve_position {
        i64::from(placement.x).clamp(i64::from(monitor.x), max_x)
    } else {
        i64::from(monitor.x) + i64::from(monitor.width - physical_width) / 2
    };
    let y = if preserve_position {
        i64::from(placement.y).clamp(i64::from(monitor.y), max_y)
    } else {
        i64::from(monitor.y) + i64::from(monitor.height - physical_height) / 2
    };

    (
        PhysicalPosition::new(
            i32::try_from(x).unwrap_or(if x < 0 { i32::MIN } else { i32::MAX }),
            i32::try_from(y).unwrap_or(if y < 0 { i32::MIN } else { i32::MAX }),
        ),
        LogicalSize::new(
            f64::from(physical_width) / scale,
            f64::from(physical_height) / scale,
        ),
    )
}

pub(crate) fn restore_window_placement(
    window: &slint::Window,
    placement: WindowPlacementPreference,
) {
    let fitted = window
        .with_winit_window(|window| {
            let monitors = window.available_monitors().collect::<Vec<_>>();
            let intersecting = monitors
                .iter()
                .map(|monitor| {
                    let geometry = monitor_geometry(monitor);
                    (geometry, placement_intersection_area(placement, geometry))
                })
                .max_by_key(|(_, area)| *area)
                .filter(|(_, area)| *area > 0);
            let fallback = window
                .current_monitor()
                .or_else(|| window.primary_monitor())
                .or_else(|| monitors.first().cloned())
                .as_ref()
                .map(monitor_geometry);
            let (monitor, preserve_position) = intersecting
                .map(|(monitor, _)| (monitor, true))
                .or_else(|| {
                    fallback.map(|monitor| (monitor, monitor.contains(placement.x, placement.y)))
                })?;
            Some(fit_window_placement(placement, monitor, preserve_position))
        })
        .flatten();

    if let Some((position, size)) = fitted {
        // Use Slint's window state as the source of truth. Direct winit resize
        // requests can be replaced by the root component's preferred size on
        // the first layout pass.
        window.set_position(slint::PhysicalPosition::new(position.x, position.y));
        window.set_size(slint::LogicalSize::new(
            size.width as f32,
            size.height as f32,
        ));
    }
}

pub(crate) fn remember_window_placement(
    app: &AppWindow,
    settings: &Arc<Mutex<AppSettings>>,
    event_position: Option<PhysicalPosition<i32>>,
    event_size: Option<PhysicalSize<u32>>,
) {
    let previous = settings
        .lock()
        .ok()
        .and_then(|settings| settings.window_placement());
    if let Some(placement) = current_window_placement(app, previous, event_position, event_size)
        && let Ok(mut settings) = settings.lock()
    {
        settings.set_window_placement(placement);
    }
}

fn current_window_placement(
    app: &AppWindow,
    previous: Option<WindowPlacementPreference>,
    event_position: Option<PhysicalPosition<i32>>,
    event_size: Option<PhysicalSize<u32>>,
) -> Option<WindowPlacementPreference> {
    if app.get_fullscreen_mode() || app.get_picture_in_picture() || app.get_window_maximized() {
        return None;
    }

    app.window()
        .with_winit_window(|window| {
            if window.fullscreen().is_some()
                || window.is_maximized()
                || window.is_minimized().unwrap_or(false)
            {
                return None;
            }
            // During a winit callback, querying the window can still return
            // geometry from the preceding native event. Prefer the event's
            // authoritative value for the dimension that changed.
            let fallback_position = window.outer_position().ok()?;
            let fallback_size = window.inner_size().to_logical::<f64>(window.scale_factor());
            let event_size = event_size
                .map(|size| size.to_logical::<f64>(window.scale_factor()))
                .map(|size| (size.width, size.height));
            merge_window_placement(
                previous,
                event_position.map(|position| (position.x, position.y)),
                event_size,
                (fallback_position.x, fallback_position.y),
                (fallback_size.width, fallback_size.height),
            )
        })
        .flatten()
}

pub(crate) fn merge_window_placement(
    previous: Option<WindowPlacementPreference>,
    event_position: Option<(i32, i32)>,
    event_size: Option<(f64, f64)>,
    fallback_position: (i32, i32),
    fallback_size: (f64, f64),
) -> Option<WindowPlacementPreference> {
    let (x, y) = event_position
        .or_else(|| previous.map(|placement| (placement.x, placement.y)))
        .unwrap_or(fallback_position);
    let (width, height) = event_size
        .or_else(|| {
            previous.map(|placement| (f64::from(placement.width), f64::from(placement.height)))
        })
        .unwrap_or(fallback_size);
    if !width.is_finite() || !height.is_finite() {
        return None;
    }

    WindowPlacementPreference::new(x, y, width.round() as u32, height.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::{
        MonitorGeometry, fit_window_placement, merge_window_placement, placement_intersection_area,
    };
    use crate::settings::WindowPlacementPreference;

    #[test]
    fn missing_monitor_centers_saved_window_on_the_fallback_display() {
        let placement = WindowPlacementPreference::new(4_000, 2_000, 1_280, 760).unwrap();
        let monitor = MonitorGeometry {
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080,
            scale: 1.0,
        };

        assert_eq!(placement_intersection_area(placement, monitor), 0);
        let (position, size) = fit_window_placement(placement, monitor, false);
        assert_eq!((position.x, position.y), (320, 160));
        assert_eq!((size.width, size.height), (1_280.0, 760.0));
    }

    #[test]
    fn visible_saved_window_is_clamped_inside_negative_dpi_scaled_monitor() {
        let placement = WindowPlacementPreference::new(-2_600, -20, 2_000, 1_000).unwrap();
        let monitor = MonitorGeometry {
            x: -2_560,
            y: 0,
            width: 2_560,
            height: 1_440,
            scale: 1.25,
        };

        assert!(placement_intersection_area(placement, monitor) > 0);
        let (position, size) = fit_window_placement(placement, monitor, true);
        assert_eq!((position.x, position.y), (-2_560, 0));
        assert_eq!((size.width, size.height), (2_000.0, 1_000.0));
    }

    #[test]
    fn move_and_resize_events_cannot_replace_each_other_with_stale_geometry() {
        let previous = WindowPlacementPreference::new(120, 80, 980, 620);
        let moved =
            merge_window_placement(previous, Some((420, 240)), None, (0, 0), (1_280.0, 760.0));
        assert_eq!(moved, WindowPlacementPreference::new(420, 240, 980, 620));

        let resized = merge_window_placement(
            moved,
            None,
            Some((1_100.0, 680.0)),
            (0, 0),
            (1_280.0, 760.0),
        );
        assert_eq!(
            resized,
            WindowPlacementPreference::new(420, 240, 1_100, 680)
        );
    }
}
