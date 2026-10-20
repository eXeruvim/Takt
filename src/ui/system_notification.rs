use std::{
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::HWND,
    UI::{
        Shell::{
            NIF_ICON, NIF_INFO, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
            NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            HICON, ICON_BIG, ICON_SMALL, ICON_SMALL2, IDI_APPLICATION, LoadIconW, SendMessageW,
            WM_GETICON,
        },
    },
};

const TAKT_NOTIFICATION_ID: u32 = 0x5441_4b54;
const NOTIFICATION_LIFETIME: Duration = Duration::from_secs(10);
static NOTIFICATION_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub enum NotificationKind {
    Information,
    Error,
}

pub fn show(hwnd: usize, title: &str, message: &str, kind: NotificationKind) -> bool {
    if hwnd == 0 {
        return false;
    }

    let mut data = notification_data(hwnd);
    data.uFlags = NIF_ICON | NIF_TIP;
    data.hIcon = window_icon(hwnd as HWND);
    write_wide(&mut data.szTip, "Takt");

    // Adding an already registered id can fail while a previous notification is visible.
    // NIM_MODIFY below intentionally updates that existing icon in this case.
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &data);
    }

    data.uFlags = NIF_INFO;
    data.dwInfoFlags = match kind {
        NotificationKind::Information => NIIF_INFO,
        NotificationKind::Error => NIIF_ERROR,
    };
    write_wide(&mut data.szInfoTitle, title);
    write_wide(&mut data.szInfo, message);

    let displayed = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } != 0;
    if displayed {
        let generation = NOTIFICATION_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
        thread::spawn(move || {
            thread::sleep(NOTIFICATION_LIFETIME);
            if NOTIFICATION_GENERATION.load(Ordering::Acquire) == generation {
                remove(hwnd);
            }
        });
    } else {
        remove(hwnd);
    }

    displayed
}

fn window_icon(hwnd: HWND) -> HICON {
    for size in [ICON_SMALL2, ICON_SMALL, ICON_BIG] {
        let icon = unsafe { SendMessageW(hwnd, WM_GETICON, size as usize, 0) } as HICON;
        if !icon.is_null() {
            return icon;
        }
    }

    unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) }
}

pub fn clear(hwnd: usize) {
    NOTIFICATION_GENERATION.fetch_add(1, Ordering::AcqRel);
    remove(hwnd);
}

fn notification_data(hwnd: usize) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd as HWND,
        uID: TAKT_NOTIFICATION_ID,
        ..Default::default()
    }
}

fn remove(hwnd: usize) {
    if hwnd == 0 {
        return;
    }
    let data = notification_data(hwnd);
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn write_wide<const N: usize>(target: &mut [u16; N], value: &str) {
    target.fill(0);
    for (slot, code_unit) in target
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(value.encode_utf16())
    {
        *slot = code_unit;
    }
}

#[cfg(test)]
mod tests {
    use super::write_wide;

    #[test]
    fn wide_notification_text_is_terminated_and_truncated_safely() {
        let mut target = [0xffff; 5];
        write_wide(&mut target, "Takt player");

        assert_eq!(
            &target[..4],
            &[b'T' as u16, b'a' as u16, b'k' as u16, b't' as u16]
        );
        assert_eq!(target[4], 0);
    }
}
