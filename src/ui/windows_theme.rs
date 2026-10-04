use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
};

pub(super) fn prefers_dark_apps() -> Option<bool> {
    light_theme_flag("AppsUseLightTheme")
        .or_else(|| light_theme_flag("SystemUsesLightTheme"))
        .map(|uses_light_theme| !uses_light_theme)
}

fn light_theme_flag(value_name: &str) -> Option<bool> {
    let subkey = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let value_name = format!("{value_name}\0").encode_utf16().collect::<Vec<_>>();
    let mut value = 1_u32;
    let mut value_size = std::mem::size_of::<u32>() as u32;

    // SAFETY: Both strings are NUL-terminated for the duration of the call, and
    // the output buffer points to a writable u32 with its exact byte length.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(value).cast(),
            std::ptr::addr_of_mut!(value_size),
        )
    };

    (status == ERROR_SUCCESS).then_some(value != 0)
}
