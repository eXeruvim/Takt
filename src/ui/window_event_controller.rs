//! Owns native window input, geometry, focus, drag-and-drop, and keyboard routing.

#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[cfg(target_os = "windows")]
use crate::ui::{
    window_interaction::{
        WindowDragState, begin_manual_window_drag, finish_manual_window_drag,
        update_manual_window_drag,
    },
    window_placement::remember_window_placement,
    windows_theme::prefers_dark_apps as windows_prefers_dark_apps,
};
use crate::{
    ipc::Command,
    player::{commands::CommandSender, playlist::is_media_file},
    settings::{AppSettings, ThemePreference},
};
use slint::ComponentHandle;
use slint::winit_030::{
    EventResult, WinitWindowAccessor,
    winit::{
        event::{MouseButton, WindowEvent},
        keyboard::{KeyCode, ModifiersState, PhysicalKey},
        window::Theme as WinitTheme,
    },
};

use super::{shortcut_controller::ShortcutController, ui_logic::AppWindow};

#[derive(Clone, Copy)]
enum DrawerSwipeTarget {
    Info,
    Playlist,
    Settings,
}

pub(super) fn register_callbacks(
    app: &AppWindow,
    tx: &CommandSender,
    settings: &Arc<Mutex<AppSettings>>,
    shortcuts: &ShortcutController,
    #[cfg(target_os = "windows")] window_drag_state: &Arc<Mutex<WindowDragState>>,
) {
    register_window_drag(
        app,
        #[cfg(target_os = "windows")]
        window_drag_state,
    );

    let tx = tx.clone();
    let app_weak = app.as_weak();
    let shortcuts = shortcuts.clone();
    #[cfg(target_os = "windows")]
    let event_window_drag_state = Arc::clone(window_drag_state);
    #[cfg(target_os = "windows")]
    let event_settings = Arc::clone(settings);
    let mut modifiers = ModifiersState::empty();
    let mut last_cursor_position = None::<(f64, f64)>;
    let mut drawer_swipe = None::<(DrawerSwipeTarget, f64)>;
    let mut theme_initialized = false;
    #[cfg(target_os = "windows")]
    let mut fullscreen_escape_blocked_until = None::<Instant>;

    app.window().on_winit_window_event(move |window, event| {
        let Some(app) = app_weak.upgrade() else {
            return EventResult::Propagate;
        };

        if !theme_initialized {
            let system_dark_theme = window
                .with_winit_window(|native_window| {
                    native_window.theme().map(|theme| theme == WinitTheme::Dark)
                })
                .flatten()
                .or_else(system_prefers_dark_apps)
                .unwrap_or(app.get_system_dark_theme());
            app.set_system_dark_theme(system_dark_theme);
            let _ = tx.send(Command::SetDarkTheme(app.get_dark_theme()));
            theme_initialized = true;
        }

        match event {
            WindowEvent::CursorMoved { position, .. } => {
                #[cfg(target_os = "windows")]
                update_manual_window_drag(window, &event_window_drag_state);

                let scale = f64::from(app.window().scale_factor());
                let window_height = f64::from(app.window().size().height);
                let top_zone = f64::from(app.get_controls_top_zone()) * scale;
                let bottom_zone = f64::from(app.get_controls_bottom_zone()) * scale;
                app.set_controls_pointer_over(
                    position.y <= top_zone || position.y >= window_height - bottom_zone,
                );
                let current_position = (position.x, position.y);
                if last_cursor_position != Some(current_position) {
                    last_cursor_position = Some(current_position);
                    app.invoke_reveal_controls();
                }
                if let Some((target, start_x)) = drawer_swipe {
                    let delta = position.x - start_x;
                    let threshold = 48.0 * scale;
                    let should_close = match target {
                        DrawerSwipeTarget::Info | DrawerSwipeTarget::Playlist => delta < -threshold,
                        DrawerSwipeTarget::Settings => delta > threshold,
                    };
                    if should_close {
                        drawer_swipe = None;
                        match target {
                            DrawerSwipeTarget::Info => app.invoke_toggle_info(),
                            DrawerSwipeTarget::Playlist => app.invoke_toggle_playlist(),
                            DrawerSwipeTarget::Settings => app.invoke_toggle_settings(),
                        }
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                app.set_controls_pointer_over(false);
            }
            WindowEvent::Focused(focused) => {
                #[cfg(target_os = "windows")]
                if app.get_fullscreen_mode() && (!focused || !app.get_window_active()) {
                    // Windows overlays (for example Snipping Tool) may return
                    // the Escape that dismissed them before or together with
                    // the focus-restored event. Start the guard on focus loss
                    // and extend it on return so that same key press cannot
                    // also leave Takt fullscreen.
                    fullscreen_escape_blocked_until =
                        Some(Instant::now() + Duration::from_millis(750));
                }
                app.set_window_active(*focused);
                #[cfg(target_os = "windows")]
                if !focused {
                    finish_manual_window_drag(&event_window_drag_state);
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                app.set_system_dark_theme(matches!(theme, WinitTheme::Dark));
                if app.get_theme_mode() == ThemePreference::System.ui_index() {
                    let _ = tx.send(Command::SetDarkTheme(app.get_dark_theme()));
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                // Keep Slint and the native mpv child on the same frame. Resizing the child
                // here makes video geometry lead the FemtoVG control deck by one paint.
                // The AfterRendering notifier commits both dimensions together.
                #[cfg(target_os = "windows")]
                remember_window_placement(&app, &event_settings, None, Some(*size));
                // This filter runs before Slint's winit backend applies the
                // authoritative size. Requesting a redraw here queues a frame
                // with the previous layout. Slint schedules the correct frame
                // after resize_event updates the window and renderer surface.
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                app.set_window_scale_factor(*scale_factor as f32);
                window.request_redraw();
            }
            WindowEvent::Moved(position) => {
                #[cfg(target_os = "windows")]
                remember_window_placement(&app, &event_settings, Some(*position), None);
                // Moving a window does not change its contents. In particular,
                // resizing from the left/top produces Moved immediately before
                // Resized; requesting a frame here presents the old surface
                // geometry between two authoritative resize frames. The native
                // size/move timer already keeps video advancing while Windows is
                // running its modal interaction loop.
            }
            WindowEvent::CloseRequested => {
                #[cfg(target_os = "windows")]
                if let Ok(mut settings) = event_settings.lock()
                    && let Err(error) = settings.save_if_dirty()
                {
                    eprintln!("Не удалось сохранить положение окна: {error}");
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if state.is_pressed() {
                    let Some((pointer_x, pointer_y)) = last_cursor_position else {
                        return EventResult::Propagate;
                    };
                    let scale = f64::from(app.window().scale_factor());
                    let window_size = app.window().size();
                    let window_width = f64::from(window_size.width);
                    let window_height = f64::from(window_size.height);
                    let logical_width = window_width / scale;
                    let panel_top = f64::from(app.get_controls_top_zone()) * scale;
                    let panel_bottom =
                        window_height - (f64::from(app.get_controls_bottom_zone()) + 4.0) * scale;

                    if pointer_y >= panel_top && pointer_y <= panel_bottom {
                        let inspector_width = logical_width.min(392.0 + 32.0) - 32.0;
                        let (inspector_start, inspector_end) = if app.get_inspector_left_aligned() {
                            (16.0 * scale, (16.0 + inspector_width) * scale)
                        } else {
                            (
                                (logical_width - inspector_width - 16.0) * scale,
                                (logical_width - 16.0) * scale,
                            )
                        };

                        let target = if pointer_x >= inspector_start && pointer_x <= inspector_end {
                            if app.get_playlist_drawer_visible() {
                                Some(DrawerSwipeTarget::Playlist)
                            } else if app.get_info_drawer_visible() {
                                Some(DrawerSwipeTarget::Info)
                            } else if app.get_settings_drawer_visible() {
                                Some(DrawerSwipeTarget::Settings)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        drawer_swipe = target.map(|target| (target, pointer_x));
                    }
                } else {
                    drawer_swipe = None;
                    #[cfg(target_os = "windows")]
                    finish_manual_window_drag(&event_window_drag_state);
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                modifiers = new_modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return EventResult::Propagate;
                };

                #[cfg(target_os = "windows")]
                if code == KeyCode::Escape && finish_manual_window_drag(&event_window_drag_state) {
                    return EventResult::PreventDefault;
                }

                if app.get_resume_prompt_visible() {
                    if code == KeyCode::Enter {
                        app.invoke_resume_saved_playback();
                    } else if code == KeyCode::Escape {
                        app.invoke_restart_saved_playback();
                    }
                    return EventResult::PreventDefault;
                }

                if app.get_clear_playback_data_prompt_visible() {
                    if code == KeyCode::Escape {
                        app.set_clear_playback_data_prompt_visible(false);
                        return EventResult::PreventDefault;
                    }
                    return EventResult::Propagate;
                }

                if app.get_reset_settings_prompt_visible() {
                    if code == KeyCode::Escape {
                        app.set_reset_settings_prompt_visible(false);
                        return EventResult::PreventDefault;
                    }
                    return EventResult::Propagate;
                }

                if app.get_converter_visible() {
                    if code == KeyCode::Escape {
                        app.set_converter_visible(false);
                    }
                    return EventResult::PreventDefault;
                }

                if let Some(result) =
                    shortcuts.handle_capture_or_palette(&app, code, modifiers, event.repeat)
                {
                    return result;
                }

                if code == KeyCode::Escape && app.get_inspector_visible() {
                    if app.get_playlist_drawer_visible() {
                        app.invoke_toggle_playlist();
                    } else if app.get_info_drawer_visible() {
                        app.invoke_toggle_info();
                    } else if app.get_settings_drawer_visible() {
                        app.invoke_toggle_settings();
                    }
                    return EventResult::PreventDefault;
                }

                if code == KeyCode::Escape && app.get_fullscreen_mode() {
                    #[cfg(target_os = "windows")]
                    if !app.get_window_active()
                        || fullscreen_escape_blocked_until
                            .is_some_and(|deadline| Instant::now() < deadline)
                    {
                        return EventResult::PreventDefault;
                    }
                    app.invoke_toggle_fullscreen();
                    return EventResult::PreventDefault;
                }

                if let Some(result) =
                    shortcuts.dispatch_binding(&app, code, modifiers, event.repeat)
                {
                    return result;
                }
            }
            WindowEvent::HoveredFile(path) => {
                app.set_drop_target_visible(path.is_dir() || is_media_file(path));
            }
            WindowEvent::HoveredFileCancelled => {
                app.set_drop_target_visible(false);
            }
            WindowEvent::DroppedFile(path) => {
                app.set_drop_target_visible(false);
                match command_for_dropped_path(path) {
                    Ok(command) => {
                        let _ = tx.send(command);
                    }
                    Err(message) => {
                        app.set_error_message(message.into());
                        app.set_error_visible(true);
                    }
                }
            }
            _ => {}
        }

        EventResult::Propagate
    });
}

#[cfg(target_os = "windows")]
fn register_window_drag(app: &AppWindow, window_drag_state: &Arc<Mutex<WindowDragState>>) {
    let app_weak = app.as_weak();
    let window_drag_state = Arc::clone(window_drag_state);
    app.on_drag_window(move || {
        if let Some(app) = app_weak.upgrade() {
            let window = app.window();
            if app.get_fullscreen_mode() {
                return;
            }
            if !begin_manual_window_drag(window, &window_drag_state) {
                window.with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn register_window_drag(app: &AppWindow) {
    let app_weak = app.as_weak();
    app.on_drag_window(move || {
        if let Some(app) = app_weak.upgrade() {
            app.window().with_winit_window(|window| {
                let _ = window.drag_window();
            });
        }
    });
}

fn command_for_dropped_path(path: &Path) -> Result<Command, String> {
    if path.is_dir() {
        return Ok(Command::OpenFolder(path.to_owned()));
    }
    if path.is_file() && is_media_file(path) {
        return Ok(Command::OpenFile(path.to_owned()));
    }

    Err(format!(
        "Перетащенный объект не является поддерживаемым медиафайлом или папкой: {}",
        path.display()
    ))
}

#[cfg(target_os = "windows")]
fn system_prefers_dark_apps() -> Option<bool> {
    windows_prefers_dark_apps()
}

#[cfg(not(target_os = "windows"))]
fn system_prefers_dark_apps() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::command_for_dropped_path;
    use crate::ipc::Command;

    #[test]
    fn rejects_unsupported_dropped_paths() {
        assert!(command_for_dropped_path(Path::new("notes.txt")).is_err());
        assert!(command_for_dropped_path(Path::new("missing.mp4")).is_err());
    }

    #[test]
    fn creates_open_file_command_for_supported_media_drop() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        let video_path = std::env::temp_dir().join(format!("takt-drop-{unique}.mkv"));
        let audio_path = std::env::temp_dir().join(format!("takt-drop-{unique}.flac"));
        std::fs::write(&video_path, []).expect("temporary video must be created");
        std::fs::write(&audio_path, []).expect("temporary audio must be created");

        assert!(matches!(
            command_for_dropped_path(&video_path),
            Ok(Command::OpenFile(_))
        ));
        assert!(matches!(
            command_for_dropped_path(&audio_path),
            Ok(Command::OpenFile(_))
        ));

        std::fs::remove_file(video_path).expect("temporary video must be removed");
        std::fs::remove_file(audio_path).expect("temporary audio must be removed");
    }
}
