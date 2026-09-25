//! Native non-client policy for Oxide's custom chrome, including transitions.
#![deny(clippy::undocumented_unsafe_blocks)]

use anyhow::{Result, anyhow, ensure};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, SetLastError, WPARAM},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect},
    System::Threading::GetCurrentThreadId,
    UI::{
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, GetWindowThreadProcessId, IsZoomed,
            NCCALCSIZE_PARAMS, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SetWindowLongW, SetWindowPos, WM_NCACTIVATE, WM_NCCALCSIZE,
            WM_NCDESTROY, WM_NCPAINT, WM_STYLECHANGING, WS_CAPTION, WS_EX_CLIENTEDGE,
            WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE,
        },
    },
};

const SUBCLASS_ID: usize = 0x4f58_4652;
// Undocumented legacy frame-paint messages also filtered by Chromium's
// HWNDMessageHandler::OnNCUAHDrawCaption/OnNCUAHDrawFrame. Keep them isolated here.
const WM_NCUAHDRAWCAPTION: u32 = 0x00ae;
const WM_NCUAHDRAWFRAME: u32 = 0x00af;

/// Install/update on the owning UI thread, BEFORE native fullscreen changes.
/// Call only from the running event loop: Slint's show() alone does not create
/// the native window, so querying its handle before run_event_loop() fails.
/// Initial installation runs in RenderingSetup, during Slint/winit's hidden
/// first-frame render, before set_visible(true) can expose a system caption.
/// The HWND owns this registration until WM_NCDESTROY. ref_data is a boolean,
/// never a Rust pointer, so no callback can outlive borrowed application state.
pub(super) fn prepare(window: &slint::Window, fullscreen: bool) -> Result<()> {
    let handle = window.window_handle();
    let handle = handle
        .window_handle()
        .map_err(|error| anyhow!("Дескриптор окна: {error}"))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        anyhow::bail!("Ожидалось окно Win32");
    };
    let hwnd = handle.hwnd.get() as HWND;
    // SAFETY: Slint owns the live HWND; prepare_hwnd also checks its thread.
    unsafe { prepare_hwnd(hwnd, fullscreen) }
}

/// The caller keeps this HWND alive until registration completes.
unsafe fn prepare_hwnd(hwnd: HWND, fullscreen: bool) -> Result<()> {
    // SAFETY: HWND is borrowed from the live Slint window. Both calls only query
    // thread identifiers; the optional process-ID output pointer may be null.
    let owning_thread =
        unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) == GetCurrentThreadId() };
    ensure!(
        owning_thread,
        "Оформление окна можно менять только в UI-потоке"
    );
    let mut old_data = 0;
    // SAFETY: live HWND on this thread, static callback, writable output integer.
    let installed =
        unsafe { GetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, &mut old_data) != 0 };
    // SAFETY: static callback and integer state; repeated installation with the
    // same callback/ID updates ref_data, rather than adding another subclass.
    let registered = unsafe {
        SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, usize::from(fullscreen)) != 0
    };
    ensure!(registered, "Не удалось подключить обработку рамки окна");
    // SAFETY: live HWND on its owning thread, checked above. Install the style
    // filter first, then remove the caption/edges already present on the HWND.
    let styles_changed = match unsafe { apply_chrome_styles(hwnd) } {
        Ok(changed) => changed,
        Err(error) => {
            if !installed {
                // SAFETY: roll back only this call's registration.
                unsafe { RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID) };
            }
            return Err(error);
        }
    };
    if !installed || styles_changed {
        // SAFETY: commit the new client-area policy without moving, activating,
        // resizing or changing the z-order. No Rust pointers are retained.
        let changed = unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            )
        };
        if changed == 0 {
            let error = std::io::Error::last_os_error();
            if !installed {
                // SAFETY: roll back only the registration created in this call.
                unsafe {
                    RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID);
                }
            }
            return Err(error.into());
        }
    }
    Ok(())
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    fullscreen: usize,
) -> LRESULT {
    match message {
        WM_STYLECHANGING if lparam != 0 => {
            // SAFETY: preserve the lower subclass chain first. No Rust borrow
            // of STYLESTRUCT is held across this potentially reentrant call.
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            // SAFETY: WM_STYLECHANGING supplies writable STYLESTRUCT storage
            // until this synchronous callback returns. Only styleNew changes.
            unsafe {
                let styles = &mut *(lparam as *mut STYLESTRUCT);
                styles.styleNew = chrome_style(wparam as i32, styles.styleNew);
            }
            0
        }
        WM_NCCALCSIZE if lparam != 0 => {
            // SAFETY: Windows supplies NCCALCSIZE_PARAMS when wParam != 0 and
            // RECT otherwise, writable for this synchronous message only.
            // Do not hold a Rust reference across calls into the window system.
            let rect_ptr = if wparam != 0 {
                // SAFETY: this message variant provides NCCALCSIZE_PARAMS;
                // its first rectangle is the proposed client area.
                unsafe { std::ptr::addr_of_mut!((*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]) }
            } else {
                lparam as *mut RECT
            };
            // SAFETY: pointer validated by the WM_NCCALCSIZE contract above.
            let proposed = unsafe { *rect_ptr };
            // SAFETY: query the live HWND supplied to the callback.
            let maximized = unsafe { IsZoomed(hwnd) != 0 };
            let work_area = if maximized && fullscreen == 0 {
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                // SAFETY: proposed rect and monitor-info storage live for these
                // calls. Select the destination monitor, including negative origins.
                let found = unsafe {
                    let monitor = MonitorFromRect(&proposed, MONITOR_DEFAULTTONEAREST);
                    GetMonitorInfoW(monitor, &mut info) != 0
                };
                found.then_some(info.rcWork)
            } else {
                None
            };
            // SAFETY: same writable message storage; no pointer escapes.
            unsafe {
                *rect_ptr = client_rect(proposed, fullscreen != 0, work_area);
            }
            // Custom chrome owns the whole client area, even if Slint temporarily
            // enables decorations. Do not let DefWindowProc reserve a caption.
            0
        }
        WM_NCPAINT | WM_NCUAHDRAWCAPTION | WM_NCUAHDRAWFRAME => 0,
        WM_NCACTIVATE => {
            // SAFETY: preserve winit's active/focus bookkeeping. Documented -1
            // tells DefWindowProc to update activation WITHOUT painting a caption.
            unsafe { DefSubclassProc(hwnd, message, wparam, -1) }
        }
        WM_NCDESTROY => {
            // SAFETY: remove only our static registration while HWND is live;
            // forward destruction so winit and the remaining chain can clean up.
            unsafe {
                RemoveWindowSubclass(hwnd, Some(frame_proc), subclass_id);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        _ => {
            // SAFETY: unchanged message/parameters belong to this callback.
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
    }
}

// Keep operational flags (resize, maximize, minimize, taskbar, visibility,
// transparency) owned by winit. Only custom-chrome decorations are filtered.
fn chrome_style(index: i32, style: u32) -> u32 {
    match index {
        GWL_STYLE => style & !WS_CAPTION,
        // Windows derives WS_EX_WINDOWEDGE from WS_THICKFRAME. Keep that
        // resize-related bit; trying to clear it is immediately normalized
        // back by user32. Only independently configurable edges are removed.
        GWL_EXSTYLE => style & !(WS_EX_CLIENTEDGE | WS_EX_STATICEDGE | WS_EX_DLGMODALFRAME),
        _ => style,
    }
}

/// Caller guarantees a live HWND on its owning thread.
unsafe fn apply_chrome_styles(hwnd: HWND) -> Result<bool> {
    let mut changed = false;
    for index in [GWL_STYLE, GWL_EXSTYLE] {
        // SAFETY: caller's HWND/thread contract; style fields are 32-bit on
        // both Win32 and Win64. Clear last-error to distinguish a zero style.
        let style = unsafe {
            SetLastError(0);
            let style = GetWindowLongW(hwnd, index);
            if style == 0 && GetLastError() != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            style as u32
        };
        let desired = chrome_style(index, style);
        if desired != style {
            // SAFETY: only style fields of the caller's HWND are changed.
            // The subclass filters future backend writes by the same policy.
            unsafe {
                SetLastError(0);
                if SetWindowLongW(hwnd, index, desired as i32) == 0 && GetLastError() != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let actual = GetWindowLongW(hwnd, index) as u32;
                ensure!(
                    actual == desired,
                    "Стиль {index}: запрошен 0x{desired:08x}, получен 0x{actual:08x}"
                );
            }
            changed = true;
        }
    }
    Ok(changed)
}
fn client_rect(proposed: RECT, fullscreen: bool, maximized_work_area: Option<RECT>) -> RECT {
    if fullscreen {
        proposed
    } else {
        maximized_work_area.unwrap_or(proposed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowTextW, ICON_BIG, IDI_APPLICATION, IsWindowVisible,
        LoadIconW, SendMessageW, SetWindowTextW, WM_GETICON, WM_SETICON, WS_CLIPSIBLINGS,
        WS_EX_ACCEPTFILES, WS_EX_APPWINDOW, WS_EX_WINDOWEDGE, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW,
        WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    struct HiddenTestWindow(HWND);

    impl Drop for HiddenTestWindow {
        fn drop(&mut self) {
            // SAFETY: test creates and destroys this HWND on the same thread;
            // it is never shown and no handle escapes the test.
            unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn hidden_hwnd_rejects_backend_caption_rewrites_and_keeps_metadata() {
        // SAFETY: built-in STATIC class, static UTF-16 strings, null optional
        // handles. Crucially, no WS_VISIBLE or ShowWindow: this is headless.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_WINDOWEDGE,
                windows_sys::core::w!("STATIC"),
                windows_sys::core::w!("frame test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                64,
                64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "{}", std::io::Error::last_os_error());
        let _window = HiddenTestWindow(hwnd);
        // SAFETY: live, hidden HWND owned by this test thread. All message
        // buffers remain alive for synchronous calls; the shared stock icon
        // is borrowed, never freed. There is no message pump or GUI launch.
        unsafe {
            prepare_hwnd(hwnd, false).unwrap();
            assert_eq!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION, 0);
            for style in [
                WS_OVERLAPPEDWINDOW | WS_CLIPSIBLINGS,
                WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPSIBLINGS,
                WS_OVERLAPPEDWINDOW | WS_CLIPSIBLINGS,
            ] {
                SetWindowLongW(hwnd, GWL_STYLE, style as i32);
                let actual = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                assert_eq!(actual, style & !WS_CAPTION);
                assert_eq!(actual & WS_THICKFRAME, style & WS_THICKFRAME);
            }
            let operational = WS_EX_APPWINDOW | WS_EX_ACCEPTFILES;
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                (operational
                    | WS_EX_WINDOWEDGE
                    | WS_EX_CLIENTEDGE
                    | WS_EX_STATICEDGE
                    | WS_EX_DLGMODALFRAME) as i32,
            );
            assert_eq!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
                operational | WS_EX_WINDOWEDGE
            );

            let title = windows_sys::core::w!("Oxide metadata test");
            assert_ne!(SetWindowTextW(hwnd, title), 0);
            let mut text = [0u16; 64];
            let count = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
            assert_eq!(
                String::from_utf16_lossy(&text[..count as usize]),
                "Oxide metadata test"
            );
            let icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
            assert!(!icon.is_null());
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
            assert_eq!(
                SendMessageW(hwnd, WM_GETICON, ICON_BIG as usize, 0),
                icon as isize
            );
            for active in [1, 0, 1, 0] {
                SendMessageW(hwnd, WM_NCACTIVATE, active, 0);
                assert_eq!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION, 0);
                assert_eq!(IsWindowVisible(hwnd), 0);
            }
        }
    }

    fn coords(rect: RECT) -> [i32; 4] {
        [rect.left, rect.top, rect.right, rect.bottom]
    }

    #[test]
    fn maximized_client_excludes_taskbar_and_invisible_resize_overhang() {
        let proposed = RECT {
            left: -1928,
            top: -8,
            right: 8,
            bottom: 1048,
        };
        let work = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        assert_eq!(
            coords(client_rect(proposed, false, Some(work))),
            [-1920, 0, 0, 1040]
        );
    }

    #[test]
    fn fullscreen_does_not_inherit_maximized_work_area() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let work = RECT {
            left: -1872,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(
            coords(client_rect(monitor, true, Some(work))),
            [-1920, 0, 0, 1080]
        );
    }

    #[test]
    fn normal_window_keeps_entire_client_without_caption_inset() {
        let proposed = RECT {
            left: 180,
            top: 90,
            right: 1460,
            bottom: 850,
        };
        assert_eq!(
            coords(client_rect(proposed, false, None)),
            [180, 90, 1460, 850]
        );
    }
}
