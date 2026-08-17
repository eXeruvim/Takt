use std::{ffi::c_void, mem};

use anyhow::{anyhow, Result};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::WindowsAndMessaging::{
        GetWindowPlacement, SetWindowPlacement, SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW, WINDOWPLACEMENT,
    },
};

/// Keeps fullscreen transitions frameless on Windows.
///
/// Winit implements a frameless resizable window by retaining the standard caption styles and
/// removing the non-client area in `WM_NCCALCSIZE`. Its borderless-fullscreen exit briefly
/// restores those styles before that message is handled, allowing DWM to present one frame with
/// the standard Windows title bar. Moving the existing frameless window to the monitor bounds
/// avoids changing its styles altogether.
#[derive(Default)]
pub struct NativeFullscreen {
    saved_placement: Option<WINDOWPLACEMENT>,
}

impl NativeFullscreen {
    pub fn set(&mut self, window: &slint::Window, fullscreen: bool) -> Result<()> {
        let hwnd = window_handle(window)?;

        if fullscreen {
            self.enter(hwnd)?;
        } else {
            self.exit(hwnd)?;
        }

        window.with_winit_window(|window| window.request_redraw());
        Ok(())
    }

    pub fn set_active(window: &slint::Window, active: bool) -> Result<()> {
        let hwnd = window_handle(window)?;
        let insert_after = if active { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let updated = unsafe {
            SetWindowPos(
                hwnd,
                insert_after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        };
        if updated == 0 {
            return Err(anyhow!("Не удалось обновить положение полноэкранного окна"));
        }

        Ok(())
    }

    fn enter(&mut self, hwnd: HWND) -> Result<()> {
        if self.saved_placement.is_some() {
            return Ok(());
        }

        let mut placement = WINDOWPLACEMENT {
            length: mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
            return Err(anyhow!("Не удалось сохранить положение окна"));
        }

        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return Err(anyhow!(
                "Не удалось определить монитор для полноэкранного режима"
            ));
        }

        let mut monitor_info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0 {
            return Err(anyhow!("Не удалось получить размеры монитора"));
        }

        let bounds = monitor_info.rcMonitor;
        let updated = unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            )
        };
        if updated == 0 {
            return Err(anyhow!("Не удалось включить полноэкранный режим"));
        }

        self.saved_placement = Some(placement);
        Ok(())
    }

    fn exit(&mut self, hwnd: HWND) -> Result<()> {
        let Some(placement) = self.saved_placement.take() else {
            return Ok(());
        };

        let restored = unsafe { SetWindowPlacement(hwnd, &placement) };
        let z_order_restored = unsafe {
            SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        };
        if restored == 0 || z_order_restored == 0 {
            self.saved_placement = Some(placement);
            return Err(anyhow!("Не удалось восстановить положение окна"));
        }

        Ok(())
    }
}

fn window_handle(window: &slint::Window) -> Result<HWND> {
    let window_handle = window.window_handle();
    let raw_handle = window_handle
        .window_handle()
        .map_err(|error| anyhow!("Не удалось получить дескриптор окна: {error:?}"))?;
    let RawWindowHandle::Win32(handle) = raw_handle.as_raw() else {
        return Err(anyhow!("Ожидался Win32-дескриптор окна"));
    };

    Ok(handle.hwnd.get() as *mut c_void)
}
