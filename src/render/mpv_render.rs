#[cfg(target_os = "windows")]
mod windows {
    use std::{
        ffi::{c_void, OsStr},
        os::windows::ffi::OsStrExt,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            OnceLock,
        },
    };

    use anyhow::{anyhow, Result};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::{BeginPaint, EndPaint, GetStockObject, BLACK_BRUSH, PAINTSTRUCT},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    };

    static VIDEO_WINDOW_CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    static FULLSCREEN_REQUESTED: AtomicBool = AtomicBool::new(false);
    static POINTER_ACTIVITY_REQUESTED: AtomicBool = AtomicBool::new(false);
    static PRIMARY_CLICK_REQUESTED: AtomicBool = AtomicBool::new(false);
    static LAST_POINTER_SCREEN_POSITION: AtomicU64 = AtomicU64::new(u64::MAX);

    pub struct VideoHost {
        handle: usize,
        last_bounds: Option<(i32, i32, i32, i32)>,
    }

    impl VideoHost {
        pub fn create(window: &slint::Window) -> Result<Self> {
            let window_handle = window.window_handle();
            let raw_handle = window_handle
                .window_handle()
                .map_err(|error| anyhow!("Не удалось получить дескриптор окна: {error:?}"))?;
            let RawWindowHandle::Win32(handle) = raw_handle.as_raw() else {
                return Err(anyhow!("Ожидался Win32-дескриптор окна"));
            };

            let parent = handle.hwnd.get() as *mut c_void;
            let class_name = VIDEO_WINDOW_CLASS.get_or_init(|| to_wide_null("OxideVideoHost"));

            let child = unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let window_class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                    lpfnWndProc: Some(video_host_proc),
                    hInstance: instance,
                    hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                    hbrBackground: GetStockObject(BLACK_BRUSH) as _,
                    lpszClassName: class_name.as_ptr(),
                    ..std::mem::zeroed()
                };
                RegisterClassW(&window_class);

                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    1,
                    1,
                    parent,
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null_mut(),
                )
            };

            if child.is_null() {
                return Err(anyhow!("Не удалось создать поверхность видео"));
            }

            Ok(Self {
                handle: child as usize,
                last_bounds: None,
            })
        }

        pub fn handle(&self) -> usize {
            self.handle
        }

        pub fn resize(
            &mut self,
            window: &slint::Window,
            left_inset: f32,
            right_inset: f32,
            top_inset: f32,
            bottom_inset: f32,
        ) -> Result<()> {
            let size = window.size();
            let scale = window.scale_factor();
            let x = (left_inset * scale).round() as i32;
            let y = (top_inset * scale).round() as i32;
            let right = (right_inset * scale).round() as i32;
            let bottom = (bottom_inset * scale).round() as i32;
            let width = (size.width as i32 - x - right).max(1);
            let height = (size.height as i32 - y - bottom).max(1);
            let bounds = (x, y, width, height);

            if self.last_bounds == Some(bounds) {
                return Ok(());
            }

            let moved = unsafe {
                SetWindowPos(
                    self.handle as HWND,
                    std::ptr::null_mut(),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOCOPYBITS,
                )
            };
            if moved == 0 {
                return Err(anyhow!("Не удалось изменить размер поверхности видео"));
            }

            self.last_bounds = Some(bounds);
            Ok(())
        }

        pub fn set_visible(&self, visible: bool) {
            unsafe {
                ShowWindow(self.handle as HWND, if visible { SW_SHOW } else { SW_HIDE });
            }
        }

        pub fn take_fullscreen_request() -> bool {
            FULLSCREEN_REQUESTED.swap(false, Ordering::AcqRel)
        }

        pub fn take_pointer_activity() -> bool {
            POINTER_ACTIVITY_REQUESTED.swap(false, Ordering::AcqRel)
        }

        pub fn take_primary_click() -> bool {
            PRIMARY_CLICK_REQUESTED.swap(false, Ordering::AcqRel)
        }
    }

    impl Drop for VideoHost {
        fn drop(&mut self) {
            unsafe {
                if IsWindow(self.handle as HWND) != 0 {
                    DestroyWindow(self.handle as HWND);
                }
            }
        }
    }

    fn to_wide_null(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    fn update_pointer_position(last_position: &AtomicU64, x: i32, y: i32) -> bool {
        let packed = (u64::from(x as u32) << 32) | u64::from(y as u32);
        last_position.swap(packed, Ordering::AcqRel) != packed
    }

    fn record_pointer_activity(force: bool) {
        let mut point = POINT { x: 0, y: 0 };
        let position_changed = unsafe {
            if GetCursorPos(&mut point) == 0 {
                false
            } else {
                update_pointer_position(&LAST_POINTER_SCREEN_POSITION, point.x, point.y)
            }
        };

        if force || position_changed {
            POINTER_ACTIVITY_REQUESTED.store(true, Ordering::Release);
        }
    }

    unsafe extern "system" fn video_host_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_MOUSEMOVE => {
                record_pointer_activity(false);
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_LBUTTONDOWN => {
                record_pointer_activity(true);
                PRIMARY_CLICK_REQUESTED.store(true, Ordering::Release);
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL => {
                record_pointer_activity(true);
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_LBUTTONDBLCLK => {
                record_pointer_activity(true);
                FULLSCREEN_REQUESTED.store(true, Ordering::Release);
                0
            }
            WM_PAINT => {
                let mut paint = std::mem::zeroed::<PAINTSTRUCT>();
                BeginPaint(window, &mut paint);
                EndPaint(window, &paint);
                0
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::AtomicU64;

        use super::update_pointer_position;

        #[test]
        fn ignores_duplicate_native_mouse_move_messages() {
            let last_position = AtomicU64::new(u64::MAX);

            assert!(update_pointer_position(&last_position, 640, 480));
            assert!(!update_pointer_position(&last_position, 640, 480));
            assert!(update_pointer_position(&last_position, 641, 480));
            assert!(update_pointer_position(&last_position, -1, -20));
            assert!(!update_pointer_position(&last_position, -1, -20));
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::VideoHost;
