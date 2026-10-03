pub(crate) mod command_catalog;
pub(crate) mod event_delivery;
#[cfg(target_os = "windows")]
mod native_frame;
#[cfg(target_os = "windows")]
mod native_fullscreen;
pub(crate) mod presentation;
mod shortcut_close;
#[cfg(target_os = "windows")]
mod system_notification;
pub mod ui_logic;
