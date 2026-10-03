#![deny(clippy::undocumented_unsafe_blocks)]

use std::sync::{Arc, Mutex};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::dpi::{PhysicalPosition, PhysicalSize},
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    UI::{
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
        WindowsAndMessaging::{GetCursorPos, GetWindowPlacement, WINDOWPLACEMENT},
    },
};

#[derive(Clone, Copy, Default)]
pub(crate) struct WindowDragState {
    active: bool,
    cursor_origin: (i32, i32),
    window_origin: (i32, i32),
}

pub(crate) fn native_window_handle(window: &slint::Window) -> Option<HWND> {
    let window_handle = window.window_handle();
    let handle = window_handle.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

fn cursor_screen_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is valid writable storage for this synchronous call.
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    Some((point.x, point.y))
}

fn restored_window_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        flags: 0,
        showCmd: 0,
        ptMinPosition: POINT { x: 0, y: 0 },
        ptMaxPosition: POINT { x: 0, y: 0 },
        rcNormalPosition: Default::default(),
    };
    // SAFETY: hwnd belongs to the live Slint window and placement has the
    // required length plus writable storage.
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return None;
    }
    let rect = placement.rcNormalPosition;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    (width > 0 && height > 0).then_some((width, height))
}

fn restored_window_origin(
    cursor_origin: (i32, i32),
    current_origin: PhysicalPosition<i32>,
    current_size: PhysicalSize<u32>,
    restored_size: (i32, i32),
    scale_factor: f64,
) -> PhysicalPosition<i32> {
    let anchor_x = ((cursor_origin.0 - current_origin.x) as f64
        / f64::from(current_size.width.max(1)))
    .clamp(0.05, 0.95);
    let title_height = (48.0 * scale_factor).round() as i32;
    let anchor_y = (cursor_origin.1 - current_origin.y).clamp(0, title_height);
    PhysicalPosition::new(
        cursor_origin.0 - (f64::from(restored_size.0) * anchor_x).round() as i32,
        cursor_origin.1 - anchor_y,
    )
}

pub(crate) fn begin_manual_window_drag(
    window: &slint::Window,
    state: &Arc<Mutex<WindowDragState>>,
) -> bool {
    let Some(cursor_origin) = cursor_screen_position() else {
        return false;
    };
    let Some(hwnd) = native_window_handle(window) else {
        return false;
    };
    let Some(window_origin) = window
        .with_winit_window(|window| {
            let current_origin = window.outer_position().ok()?;
            if !window.is_maximized() {
                return Some(current_origin);
            }

            let current_size = window.outer_size();
            let restored_size = restored_window_size(hwnd)
                .unwrap_or((current_size.width as i32, current_size.height as i32));
            let restored_origin = restored_window_origin(
                cursor_origin,
                current_origin,
                current_size,
                restored_size,
                window.scale_factor(),
            );

            window.set_maximized(false);
            window.set_outer_position(restored_origin);
            Some(restored_origin)
        })
        .flatten()
    else {
        return false;
    };

    if let Ok(mut state) = state.lock() {
        *state = WindowDragState {
            active: true,
            cursor_origin,
            window_origin: (window_origin.x, window_origin.y),
        };
        // SAFETY: hwnd is the live Slint window on this UI thread. Capture is
        // released by finish_manual_window_drag on every completed drag path.
        unsafe {
            SetCapture(hwnd);
        }
        true
    } else {
        false
    }
}

pub(crate) fn update_manual_window_drag(
    window: &slint::Window,
    state: &Arc<Mutex<WindowDragState>>,
) {
    let drag = state
        .lock()
        .ok()
        .and_then(|state| state.active.then_some(*state));
    let (Some(drag), Some(cursor)) = (drag, cursor_screen_position()) else {
        return;
    };

    window.with_winit_window(|window| {
        window.set_outer_position(PhysicalPosition::new(
            drag.window_origin.0 + cursor.0 - drag.cursor_origin.0,
            drag.window_origin.1 + cursor.1 - drag.cursor_origin.1,
        ));
    });
}

pub(crate) fn finish_manual_window_drag(state: &Arc<Mutex<WindowDragState>>) -> bool {
    let active = state
        .lock()
        .map(|mut state| std::mem::take(&mut state.active))
        .unwrap_or(false);
    if active {
        // SAFETY: Called on the UI thread that acquired mouse capture; Windows
        // ignores ReleaseCapture if ownership was already lost.
        unsafe {
            ReleaseCapture();
        }
    }
    active
}

#[cfg(test)]
mod tests {
    use super::restored_window_origin;
    use slint::winit_030::winit::dpi::{PhysicalPosition, PhysicalSize};

    #[test]
    fn dragging_a_maximized_window_keeps_the_pointer_anchor() {
        let origin = restored_window_origin(
            (960, 24),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1_920, 1_080),
            (1_280, 760),
            1.0,
        );

        assert_eq!(origin, PhysicalPosition::new(320, 0));
    }

    #[test]
    fn restored_anchor_stays_away_from_extreme_window_edges() {
        let left = restored_window_origin(
            (0, 10),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1_000, 700),
            (800, 600),
            1.0,
        );
        let right = restored_window_origin(
            (1_000, 10),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1_000, 700),
            (800, 600),
            1.0,
        );

        assert_eq!(left.x, -40);
        assert_eq!(right.x, 240);
    }
}
