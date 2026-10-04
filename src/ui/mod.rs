pub(crate) mod command_catalog;
mod conversion_controller;
pub(crate) mod event_delivery;
#[cfg(target_os = "windows")]
mod native_frame;
#[cfg(target_os = "windows")]
mod native_fullscreen;
mod player_event_controller;
pub(crate) mod presentation;
mod settings_controller;
mod shortcut_close;
#[cfg(target_os = "windows")]
mod system_notification;
pub mod ui_logic;
#[cfg(target_os = "windows")]
mod video_output;
#[cfg(target_os = "windows")]
mod window_interaction;
#[cfg(target_os = "windows")]
mod window_modes;
#[cfg(target_os = "windows")]
mod window_placement;
#[cfg(target_os = "windows")]
mod windows_theme;
