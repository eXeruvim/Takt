pub(crate) mod event_delivery;
#[cfg(target_os = "windows")]
mod native_fullscreen;
mod shortcut_close;
#[cfg(target_os = "windows")]
mod system_notification;
pub mod ui_logic;
