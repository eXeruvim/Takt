//! Native non-client policy for Oxide's custom chrome, including transitions.
#![deny(clippy::undocumented_unsafe_blocks)]

use anyhow::{Result, anyhow, ensure};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, SetLastError, WPARAM},
    Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow,
        RDW_INTERNALPAINT, RedrawWindow,
    },
    System::Threading::GetCurrentThreadId,
    UI::{
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, GetWindowThreadProcessId, IsZoomed, KillTimer,
            MINMAXINFO, NCCALCSIZE_PARAMS, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetTimer, SetWindowLongW, SetWindowPos,
            WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_NCACTIVATE, WM_NCCALCSIZE,
            WM_NCDESTROY, WM_NCPAINT, WM_STYLECHANGING, WM_TIMER, WS_CAPTION, WS_EX_CLIENTEDGE,
            WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE, WVR_VALIDRECTS,
        },
    },
};

const SUBCLASS_ID: usize = 0x4f58_4652;
const LIVE_RESIZE_TIMER_ID: usize = 0x4f58_5253;
const LIVE_RESIZE_FRAME_INTERVAL_MS: u32 = 16;
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
    let fullscreen = fullscreen != 0;
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
        WM_GETMINMAXINFO if lparam != 0 && !fullscreen => {
            // Preserve winit's minimum tracking size, then constrain only the
            // maximized outer rectangle to the monitor work area. Correcting
            // WM_NCCALCSIZE alone leaves an invisible borderless overhang that
            // can still sit above taskbar buttons and consume their clicks.
            // SAFETY: preserve the lower subclass chain for this live HWND;
            // the message parameters are forwarded unchanged.
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: hwnd is live for the callback and info is writable for
            // this synchronous monitor query.
            let found = unsafe {
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                GetMonitorInfoW(monitor, &mut info) != 0
            };
            if found {
                let (position, size) = maximized_bounds(info.rcMonitor, info.rcWork);
                // SAFETY: WM_GETMINMAXINFO supplies writable MINMAXINFO storage
                // for the duration of this call; no pointer escapes.
                unsafe {
                    let limits = &mut *(lparam as *mut MINMAXINFO);
                    limits.ptMaxPosition.x = position.0;
                    limits.ptMaxPosition.y = position.1;
                    limits.ptMaxSize.x = size.0;
                    limits.ptMaxSize.y = size.1;
                }
            }
            0
        }
        WM_NCCALCSIZE if lparam != 0 => {
            // SAFETY: Windows supplies NCCALCSIZE_PARAMS when wParam != 0 and
            // RECT otherwise, writable for this synchronous message only.
            // Do not hold a Rust reference across calls into the window system.
            let (rect_ptr, params_ptr, previous_client) = if wparam != 0 {
                // SAFETY: this message variant provides NCCALCSIZE_PARAMS;
                // its first rectangle is the proposed client area and its
                // third rectangle is the previous client area.
                unsafe {
                    let params = lparam as *mut NCCALCSIZE_PARAMS;
                    (
                        std::ptr::addr_of_mut!((*params).rgrc[0]),
                        Some(params),
                        Some((*params).rgrc[2]),
                    )
                }
            } else {
                (lparam as *mut RECT, None, None)
            };
            // SAFETY: pointer validated by the WM_NCCALCSIZE contract above.
            let proposed = unsafe { *rect_ptr };
            // SAFETY: query the live HWND supplied to the callback.
            let maximized = unsafe { IsZoomed(hwnd) != 0 };
            let work_area = if maximized && !fullscreen {
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
            let client = client_rect(proposed, fullscreen, work_area);
            // SAFETY: same writable message storage; no pointer escapes.
            unsafe { *rect_ptr = client };

            // Keep every still-visible pixel at the same screen coordinate while
            // the left/top edge moves. Alignment flags only describe an edge;
            // VALIDRECTS gives DWM the exact overlap to copy from the previous
            // surface into the new one while Slint prepares the replacement.
            if !maximized
                && !fullscreen
                && let (Some(params), Some(previous)) = (params_ptr, previous_client)
                && let Some(overlap) = preserved_client_overlap(client, previous)
            {
                // SAFETY: WM_NCCALCSIZE owns this writable structure until the
                // callback returns. For a top-level window both rectangles use
                // screen coordinates and the overlap is inside both clients.
                unsafe {
                    (*params).rgrc[1] = overlap;
                    (*params).rgrc[2] = overlap;
                }
                WVR_VALIDRECTS as LRESULT
            } else {
                0
            }
        }
        WM_NCPAINT | WM_NCUAHDRAWCAPTION | WM_NCUAHDRAWFRAME => 0,
        WM_NCACTIVATE => {
            // SAFETY: preserve winit's active/focus bookkeeping. Documented -1
            // tells DefWindowProc to update activation WITHOUT painting a caption.
            unsafe { DefSubclassProc(hwnd, message, wparam, -1) }
        }
        WM_ENTERSIZEMOVE => {
            // Windows runs a nested modal loop while the user holds a resize
            // border. Once the minimum size is reached, no more WM_SIZE events
            // arrive and the normal Slint event-loop wake-up can be starved.
            // A window timer is dispatched inside that nested loop and keeps
            // mpv's video surface presenting until the border is released.
            // SAFETY: the timer belongs to this live HWND, has no callback
            // pointer, and is stopped on both resize exit and destruction.
            unsafe {
                SetTimer(
                    hwnd,
                    LIVE_RESIZE_TIMER_ID,
                    LIVE_RESIZE_FRAME_INTERVAL_MS,
                    None,
                );
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        WM_TIMER if wparam == LIVE_RESIZE_TIMER_ID => {
            // Match winit's request_redraw implementation. RDW_INTERNALPAINT
            // coalesces redundant ticks instead of building a frame backlog.
            // SAFETY: hwnd is live for the duration of the subclass callback;
            // RedrawWindow does not retain either null region pointer.
            unsafe {
                RedrawWindow(
                    hwnd,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INTERNALPAINT,
                );
            }
            0
        }
        WM_EXITSIZEMOVE => {
            // SAFETY: removing an absent timer is harmless; forward the message
            // so winit can clear its own interactive-resize state as usual.
            unsafe {
                KillTimer(hwnd, LIVE_RESIZE_TIMER_ID);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        WM_NCDESTROY => {
            // SAFETY: remove only our static registration while HWND is live;
            // forward destruction so winit and the remaining chain can clean up.
            unsafe {
                KillTimer(hwnd, LIVE_RESIZE_TIMER_ID);
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

fn maximized_bounds(monitor: RECT, work: RECT) -> ((i32, i32), (i32, i32)) {
    (
        (work.left - monitor.left, work.top - monitor.top),
        (work.right - work.left, work.bottom - work.top),
    )
}

fn preserved_client_overlap(current: RECT, previous: RECT) -> Option<RECT> {
    let size_changed = current.right - current.left != previous.right - previous.left
        || current.bottom - current.top != previous.bottom - previous.top;
    if !size_changed {
        return None;
    }

    let overlap = RECT {
        left: current.left.max(previous.left),
        top: current.top.max(previous.top),
        right: current.right.min(previous.right),
        bottom: current.bottom.min(previous.bottom),
    };
    (overlap.left < overlap.right && overlap.top < overlap.bottom).then_some(overlap)
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
    fn maximized_outer_bounds_follow_work_area_on_any_taskbar_edge() {
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
            bottom: 1040,
        };
        assert_eq!(maximized_bounds(monitor, work), ((48, 0), (1872, 1040)));
    }

    #[test]
    fn resize_preserves_the_exact_screen_space_overlap() {
        let previous = RECT {
            left: 100,
            top: 100,
            right: 500,
            bottom: 400,
        };
        assert_eq!(
            coords(
                preserved_client_overlap(
                    RECT {
                        left: 80,
                        ..previous
                    },
                    previous,
                )
                .unwrap()
            ),
            [100, 100, 500, 400]
        );
        assert_eq!(
            coords(
                preserved_client_overlap(
                    RECT {
                        top: 80,
                        ..previous
                    },
                    previous,
                )
                .unwrap()
            ),
            [100, 100, 500, 400]
        );
        assert_eq!(
            coords(
                preserved_client_overlap(
                    RECT {
                        left: 140,
                        top: 130,
                        ..previous
                    },
                    previous,
                )
                .unwrap()
            ),
            [140, 130, 500, 400]
        );
        assert!(
            preserved_client_overlap(
                RECT {
                    left: 600,
                    top: 100,
                    right: 700,
                    bottom: 400,
                },
                previous
            )
            .is_none()
        );
        assert!(preserved_client_overlap(previous, previous).is_none());
        assert!(
            preserved_client_overlap(
                RECT {
                    left: 120,
                    top: 110,
                    right: 520,
                    bottom: 410,
                },
                previous
            )
            .is_none()
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
