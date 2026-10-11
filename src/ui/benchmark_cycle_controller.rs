//! Drives repeatable UI panel cycles only when an explicit benchmark is active.

use std::{cell::Cell, rc::Rc, time::Duration};

use slint::{ComponentHandle, Model, Timer, TimerMode};

use crate::benchmark_diagnostics;

use super::ui_logic::AppWindow;

const STEP_INTERVAL: Duration = Duration::from_millis(380);

pub(super) struct UiCycleDriver {
    _timer: Rc<Timer>,
}

pub(super) fn install(app: &AppWindow) -> Option<UiCycleDriver> {
    let requested = benchmark_diagnostics::requested_ui_cycles()?;
    let timer = Rc::new(Timer::default());
    let timer_weak = Rc::downgrade(&timer);
    let app_weak = app.as_weak();
    let phase = Rc::new(Cell::new(0_u8));
    let initialized = Rc::new(Cell::new(false));
    let phase_for_timer = Rc::clone(&phase);
    let initialized_for_timer = Rc::clone(&initialized);

    timer.start(TimerMode::Repeated, STEP_INTERVAL, move || {
        if !benchmark_diagnostics::measurement_window_open() {
            return;
        }
        let Some(app) = app_weak.upgrade() else {
            if let Some(timer) = timer_weak.upgrade() {
                timer.stop();
            }
            return;
        };
        if !app.get_media_loaded() || app.get_playlist_items().row_count() == 0 {
            return;
        }

        if !initialized_for_timer.get() {
            if app.get_playlist_drawer_visible() {
                app.invoke_toggle_playlist();
                return;
            }
            if app.get_info_drawer_visible() {
                app.invoke_toggle_info();
                return;
            }
            if app.get_settings_drawer_visible() {
                app.invoke_toggle_settings();
                return;
            }
            initialized_for_timer.set(true);
        }

        match phase_for_timer.get() {
            0 => {
                app.invoke_toggle_playlist();
                phase_for_timer.set(1);
            }
            1 => {
                app.invoke_toggle_settings();
                phase_for_timer.set(2);
            }
            _ => {
                app.invoke_toggle_settings();
                phase_for_timer.set(0);
                let completed = benchmark_diagnostics::record_ui_cycle_completed();
                if completed == requested {
                    eprintln!("UI benchmark: завершено {completed} циклов панелей");
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                } else if completed.is_multiple_of(100) {
                    eprintln!("UI benchmark: завершено {completed}/{requested} циклов");
                }
            }
        }
    });

    Some(UiCycleDriver { _timer: timer })
}
