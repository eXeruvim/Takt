use std::{ffi::c_void, mem};

use anyhow::{Result, anyhow};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use windows_sys::{
    Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        System::Com::{
            CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
        },
        UI::WindowsAndMessaging::{
            GetWindowPlacement, SWP_NOOWNERZORDER, SWP_SHOWWINDOW, SetWindowPlacement,
            SetWindowPos, WINDOWPLACEMENT,
        },
    },
    core::{BOOL, GUID, HRESULT, IUnknown_Vtbl},
};

#[repr(C)]
struct TaskbarListVtable {
    parent: IUnknown_Vtbl,
    hr_init: unsafe extern "system" fn(*mut TaskbarList) -> HRESULT,
    add_tab: unsafe extern "system" fn(*mut TaskbarList, HWND) -> HRESULT,
    delete_tab: unsafe extern "system" fn(*mut TaskbarList, HWND) -> HRESULT,
    activate_tab: unsafe extern "system" fn(*mut TaskbarList, HWND) -> HRESULT,
    set_active_alt: unsafe extern "system" fn(*mut TaskbarList, HWND) -> HRESULT,
}

#[repr(C)]
struct TaskbarList {
    vtable: *const TaskbarListVtable,
}

#[repr(C)]
struct TaskbarList2Vtable {
    parent: TaskbarListVtable,
    mark_fullscreen_window: unsafe extern "system" fn(*mut TaskbarList2, HWND, BOOL) -> HRESULT,
}

#[repr(C)]
struct TaskbarList2 {
    vtable: *const TaskbarList2Vtable,
}

const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
const IID_TASKBAR_LIST_2: GUID = GUID::from_u128(0x602d4995_b13a_429b_a66e_1935e44f4317);

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
        taskbar_mark_fullscreen(hwnd, true);
        let updated = unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            )
        };
        if updated == 0 {
            taskbar_mark_fullscreen(hwnd, false);
            return Err(anyhow!("Не удалось включить полноэкранный режим"));
        }

        self.saved_placement = Some(placement);
        Ok(())
    }

    fn exit(&mut self, hwnd: HWND) -> Result<()> {
        let Some(placement) = self.saved_placement.take() else {
            return Ok(());
        };

        taskbar_mark_fullscreen(hwnd, false);
        let restored = unsafe { SetWindowPlacement(hwnd, &placement) };
        if restored == 0 {
            self.saved_placement = Some(placement);
            return Err(anyhow!("Не удалось восстановить положение окна"));
        }

        Ok(())
    }
}

fn taskbar_mark_fullscreen(hwnd: HWND, fullscreen: bool) {
    unsafe {
        let initialize_result = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let should_uninitialize = initialize_result >= 0;
        let mut taskbar = std::ptr::null_mut::<TaskbarList2>();
        let create_result = CoCreateInstance(
            &CLSID_TASKBAR_LIST,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_TASKBAR_LIST_2,
            &mut taskbar as *mut _ as *mut _,
        );

        if create_result >= 0 && !taskbar.is_null() {
            let vtable = &*(*taskbar).vtable;
            if (vtable.parent.hr_init)(taskbar.cast()) >= 0 {
                (vtable.mark_fullscreen_window)(taskbar, hwnd, fullscreen.into());
            }
            (vtable.parent.parent.Release)(taskbar.cast());
        }

        if should_uninitialize {
            CoUninitialize();
        }
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
