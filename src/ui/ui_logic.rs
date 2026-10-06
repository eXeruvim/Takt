#![deny(clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use std::{
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};

use crate::converter::{ConverterCommand, ConverterEvent};
use crate::ipc::{Command, Event};
use crate::player::commands::{CommandSender, SendError as PlayerCommandError};
use crate::player::playlist::is_media_file;
use crate::settings::{AppSettings, ThemePreference};
use crate::ui::player_event_controller::PlayerEventController;
use crate::ui::shortcut_controller::ShortcutController;
#[cfg(target_os = "windows")]
use crate::ui::system_notification;
#[cfg(target_os = "windows")]
use crate::ui::video_output::VideoOutputController;
#[cfg(target_os = "windows")]
use crate::ui::window_interaction::{
    WindowDragState, begin_manual_window_drag, finish_manual_window_drag, native_window_handle,
    update_manual_window_drag,
};
#[cfg(target_os = "windows")]
use crate::ui::window_modes::WindowModeController;
#[cfg(target_os = "windows")]
use crate::ui::window_placement::{remember_window_placement, restore_window_placement};
#[cfg(target_os = "windows")]
use crate::ui::windows_theme::prefers_dark_apps as windows_prefers_dark_apps;
use slint::winit_030::{
    EventResult, WinitWindowAccessor,
    winit::{
        event::{MouseButton, WindowEvent},
        keyboard::{KeyCode, ModifiersState, PhysicalKey},
        window::Theme as WinitTheme,
    },
};
slint::include_modules!();

#[derive(Clone, Copy)]
enum DrawerSwipeTarget {
    Info,
    Playlist,
    Settings,
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

pub fn run(
    tx: CommandSender,
    rx: Receiver<Event>,
    command_errors: Receiver<PlayerCommandError>,
    converter_tx: Sender<ConverterCommand>,
    converter_rx: Receiver<ConverterEvent>,
    converter_control: Arc<crate::converter::ConverterControl>,
) -> Result<()> {
    let app = AppWindow::new()?;
    let error_weak = app.as_weak();
    let command_error_bridge = super::event_delivery::spawn(
        command_errors,
        |_, _| true,
        move |errors, completion| {
            let weak = error_weak.clone();
            slint::invoke_from_event_loop(move || {
                let _completion = completion;
                if let Some(app) = weak.upgrade()
                    && let Some(error) = errors.last()
                {
                    app.set_error_message(error.message().into());
                    app.set_error_visible(true);
                }
            })
            .is_ok()
        },
    )?;
    crate::ui::shortcut_close::install(&app);
    app.set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    app.set_window_scale_factor(app.window().scale_factor());
    let settings = Arc::new(Mutex::new(AppSettings::load_default()));
    #[cfg(target_os = "windows")]
    app.set_system_dark_theme(windows_prefers_dark_apps().unwrap_or(false));
    if let Ok(mut settings) = settings.lock() {
        if let Some(message) = settings.take_storage_notice() {
            eprintln!("{message}");
            app.set_error_message(message.into());
            app.set_error_visible(true);
        }
        app.set_theme_mode(settings.theme().ui_index());
        app.set_accent_mode(settings.accent().ui_index());
        app.set_volume(settings.default_volume());
        app.set_speed(settings.default_speed());
        app.set_folder_loop_enabled(settings.completion_behavior().repeats_playlist());
        app.set_hardware_decoding_enabled(settings.hardware_decoding_enabled());
        let _ = tx.send(Command::SetVolume(
            f64::from(settings.default_volume()) * 100.0,
        ));
        let _ = tx.send(Command::SetSpeed(f64::from(settings.default_speed())));
        let _ = tx.send(Command::SetFolderLoop(
            settings.completion_behavior().repeats_playlist(),
        ));
        let _ = tx.send(Command::SetHardwareDecoding(
            settings.hardware_decoding_enabled(),
        ));
    }
    super::conversion_controller::apply_presets(&app, false, "");
    let shortcuts = ShortcutController::new(&app, tx.clone(), settings.clone());
    let dialog_active = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let video_output = VideoOutputController::default();
    #[cfg(target_os = "windows")]
    let window_modes = WindowModeController::default();
    #[cfg(target_os = "windows")]
    let window_drag_state = Arc::new(Mutex::new(WindowDragState::default()));
    let player_events = PlayerEventController::new(
        #[cfg(target_os = "windows")]
        video_output.event_handle(),
        #[cfg(target_os = "windows")]
        tx.clone(),
    );
    let current_duration = player_events.duration_handle();

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let window_drag_state = window_drag_state.clone();
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
    {
        let app_weak = app.as_weak();
        app.on_drag_window(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        let shortcuts = shortcuts.clone();
        #[cfg(target_os = "windows")]
        let event_window_drag_state = window_drag_state.clone();
        #[cfg(target_os = "windows")]
        let event_settings = settings.clone();
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
                    .or_else(windows_prefers_dark_apps)
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
                            DrawerSwipeTarget::Info | DrawerSwipeTarget::Playlist => {
                                delta < -threshold
                            }
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
                        // also leave Oxide fullscreen.
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
                    {
                        remember_window_placement(&app, &event_settings, None, Some(*size));
                    }
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
                    {
                        if let Ok(mut settings) = event_settings.lock()
                            && let Err(error) = settings.save_if_dirty()
                        {
                            eprintln!("Не удалось сохранить положение окна: {error}");
                        }
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
                        let panel_bottom = window_height
                            - (f64::from(app.get_controls_bottom_zone()) + 4.0) * scale;

                        if pointer_y >= panel_top && pointer_y <= panel_bottom {
                            let inspector_width = logical_width.min(392.0 + 32.0) - 32.0;
                            let (inspector_start, inspector_end) =
                                if app.get_inspector_left_aligned() {
                                    (16.0 * scale, (16.0 + inspector_width) * scale)
                                } else {
                                    (
                                        (logical_width - inspector_width - 16.0) * scale,
                                        (logical_width - 16.0) * scale,
                                    )
                                };

                            let target =
                                if pointer_x >= inspector_start && pointer_x <= inspector_end {
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
                    if code == KeyCode::Escape
                        && finish_manual_window_drag(&event_window_drag_state)
                    {
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

    super::settings_controller::register_preference_callbacks(&app, &tx, &settings);
    super::player_command_controller::register_callbacks(
        &app,
        &tx,
        &dialog_active,
        &current_duration,
    );

    #[cfg(target_os = "windows")]
    window_modes.register_callbacks(&app);

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();

        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                let fullscreen = !app.window().is_fullscreen();
                app.set_fullscreen_mode(fullscreen);
                app.window().set_fullscreen(fullscreen);
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();

        app.on_toggle_picture_in_picture(move || {
            if let Some(app) = app_weak.upgrade()
                && app.get_media_loaded()
            {
                app.set_picture_in_picture(!app.get_picture_in_picture());
                app.invoke_reveal_controls();
            }
        });
    }

    super::drawer_controller::register_callbacks(
        &app,
        #[cfg(target_os = "windows")]
        video_output.clone(),
    );

    super::conversion_controller::register_callbacks(
        &app,
        converter_tx,
        converter_control,
        dialog_active.clone(),
    );

    #[cfg(target_os = "windows")]
    let native_init_error_rx =
        video_output.register_rendering(&app, tx.clone(), window_modes.clone())?;

    let player_bridge = player_events.spawn_bridge(&app, rx)?;

    let converter_bridge = super::conversion_controller::spawn_event_bridge(&app, converter_rx)?;

    // `AppWindow::new()` does not guarantee that the winit window already
    // exists. Show it before restoring native coordinates; the event loop has
    // not started yet, so the default bounds are not painted as an extra frame.
    #[cfg(target_os = "windows")]
    let initial_window_placement = settings
        .lock()
        .ok()
        .and_then(|settings| settings.window_placement());
    #[cfg(target_os = "windows")]
    if let Some(placement) = initial_window_placement {
        app.window().set_size(slint::LogicalSize::new(
            placement.width as f32,
            placement.height as f32,
        ));
        // Before the event loop starts, Slint stores this in winit's future
        // WindowAttributes. Calling through winit here is too early because no
        // native window exists yet.
        app.window()
            .set_position(slint::PhysicalPosition::new(placement.x, placement.y));
    }
    app.show()?;
    #[cfg(target_os = "windows")]
    if let Some(placement) = initial_window_placement {
        let app = app.as_weak();
        slint::invoke_from_event_loop(move || {
            if let Some(app) = app.upgrade() {
                // Clamp the saved position after the native frame is ready.
                restore_window_placement(app.window(), placement);
            }
        })?;
    }
    let run_result = slint::run_event_loop();
    let hide_result = app.hide();
    drop(player_bridge);
    drop(converter_bridge);
    drop(command_error_bridge);
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = native_window_handle(app.window()) {
        system_notification::clear(hwnd as usize);
    }
    if let Ok(mut settings) = settings.lock()
        && let Err(error) = settings.save_if_dirty()
    {
        eprintln!("Не удалось сохранить настройки при выходе: {error}");
    }
    run_result?;
    hide_result?;
    #[cfg(target_os = "windows")]
    if let Ok(error) = native_init_error_rx.try_recv() {
        return Err(error.context("Не удалось настроить рамку созданного окна"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::command_for_dropped_path;
    use crate::ipc::Command;
    use crate::player::shortcuts::{
        ShortcutAction, ShortcutBinding, ShortcutEdit, ShortcutRegistry,
    };
    use crate::settings::ShortcutPreference;
    use crate::ui::command_catalog::COMMANDS as PALETTE_COMMANDS;
    use crate::ui::shortcut_controller::{
        command_palette_items, command_palette_items_with_registry,
    };
    use slint::winit_030::winit::keyboard::{KeyCode, ModifiersState};

    #[test]
    fn rejects_unsupported_dropped_paths() {
        assert!(command_for_dropped_path(Path::new("notes.txt")).is_err());
        assert!(command_for_dropped_path(Path::new("missing.mp4")).is_err());
    }

    #[test]
    fn command_palette_returns_all_commands_for_an_empty_query() {
        assert_eq!(command_palette_items("").len(), PALETTE_COMMANDS.len());
        assert_eq!(command_palette_items("   ").len(), PALETTE_COMMANDS.len());
    }

    #[test]
    fn reset_hint_keeps_factory_binding_after_customization() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        let entries = command_palette_items_with_registry("mute", &shortcuts);
        assert_eq!(entries[0].shortcut, "Ctrl+B");
        assert_eq!(entries[0].default_shortcut, "M");
        let entries = command_palette_items_with_registry("", &shortcuts);
        let open = entries
            .iter()
            .find(|entry| entry.action == ShortcutAction::OpenFile as i32)
            .unwrap();
        assert_eq!(open.default_shortcut, "Ctrl+O / O");
        let settings = entries
            .iter()
            .find(|entry| entry.action == ShortcutAction::ToggleSettings as i32)
            .unwrap();
        assert_eq!(settings.default_shortcut, "не назначено");
    }

    #[test]
    fn command_palette_searches_titles_descriptions_and_aliases() {
        let mute = command_palette_items("mute");
        assert_eq!(mute.len(), 1);
        assert_eq!(mute[0].action, 5);

        let folder = command_palette_items("папку плейлист");
        assert_eq!(folder.len(), 1);
        assert_eq!(folder[0].action, 1);

        assert!(command_palette_items("несуществующая-команда").is_empty());
    }

    #[test]
    fn command_palette_marks_media_dependent_actions() {
        let open_file = command_palette_items("открыть файл");
        assert!(!open_file[0].requires_media);

        let seek = command_palette_items("перемотать назад");
        assert!(seek[0].requires_media);

        let playlist = command_palette_items("показать плейлист");
        assert!(playlist[0].requires_media);
    }

    #[test]
    fn shortcuts_are_resolved_from_physical_keys_and_exact_modifiers() {
        let shortcuts = ShortcutRegistry::default();
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::empty())),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::CONTROL)),
            Some(ShortcutAction::ToggleCommandPalette)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(
                KeyCode::KeyO,
                ModifiersState::CONTROL | ModifiersState::SHIFT
            )),
            Some(ShortcutAction::OpenFolder)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::ALT)),
            None
        );
    }

    #[test]
    fn rebind_replaces_aliases_for_an_action() {
        let mut shortcuts = ShortcutRegistry::default();
        let replacement = ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL);

        shortcuts
            .rebind(ShortcutAction::ToggleMute, replacement)
            .expect("unused shortcut must be accepted");

        assert_eq!(
            shortcuts.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            None
        );
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "Ctrl+B");
    }

    #[test]
    fn rebind_rejects_conflicts_without_changing_existing_bindings() {
        let mut shortcuts = ShortcutRegistry::default();
        let play_pause = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());

        let conflict = shortcuts
            .rebind(ShortcutAction::ToggleMute, play_pause)
            .expect_err("a shortcut assigned to another action must be rejected");

        assert_eq!(conflict.action, ShortcutAction::PlayPause);
        assert_eq!(conflict.binding, play_pause);
        assert_eq!(
            shortcuts.action_for(play_pause),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn shortcut_preview_requires_save_and_can_be_discarded() {
        let mut shortcuts = ShortcutRegistry::default();
        let mut edit = ShortcutEdit::default();
        let replacement = ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL);
        edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![replacement])
            .unwrap();
        assert_eq!(shortcuts.action_for(replacement), None);
        assert!(shortcuts.preferences().is_empty());
        // Cancelling or opening another editor discards the pending assignment.
        edit.discard();
        assert!(!edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "M");
        edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![replacement])
            .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        let restored = ShortcutRegistry::from_preferences(&shortcuts.preferences());
        assert_eq!(
            restored.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn conflicting_preview_cannot_save_an_earlier_valid_candidate() {
        let mut shortcuts = ShortcutRegistry::default();
        let mut edit = ShortcutEdit::default();
        edit.stage(
            &shortcuts,
            ShortcutAction::ToggleMute,
            vec![ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL)],
        )
        .unwrap();
        let conflict = edit
            .stage(
                &shortcuts,
                ShortcutAction::ToggleMute,
                vec![ShortcutBinding::new(
                    KeyCode::Space,
                    ModifiersState::empty(),
                )],
            )
            .unwrap_err();
        assert_eq!(conflict.action, ShortcutAction::PlayPause);
        assert!(!edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        assert!(shortcuts.preferences().is_empty());

        // A rejected candidate can be replaced in this edit without restarting capture.
        let replacement = ShortcutBinding::new(KeyCode::KeyD, ModifiersState::CONTROL);
        edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![replacement])
            .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        assert_eq!(
            shortcuts.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(
                KeyCode::Space,
                ModifiersState::empty()
            )),
            Some(ShortcutAction::PlayPause)
        );
    }

    #[test]
    fn explicit_transfer_disables_previous_owner_and_survives_reload() {
        // Exercise both orders in the settings snapshot, including a donor with aliases.
        for (recipient, donor, code) in [
            (
                ShortcutAction::PlayPause,
                ShortcutAction::ToggleMute,
                KeyCode::KeyM,
            ),
            (
                ShortcutAction::ToggleMute,
                ShortcutAction::PlayPause,
                KeyCode::Space,
            ),
        ] {
            let mut shortcuts = ShortcutRegistry::default();
            let mut edit = ShortcutEdit::default();
            let binding = ShortcutBinding::new(code, ModifiersState::empty());
            assert!(edit.stage(&shortcuts, recipient, vec![binding]).is_err());
            assert_eq!(shortcuts.action_for(binding), Some(donor));
            assert!(shortcuts.preferences().is_empty());
            assert!(edit.overwrite(&mut shortcuts, recipient));
            assert_eq!(shortcuts.action_for(binding), Some(recipient));
            assert_eq!(shortcuts.display_for(donor), "—");
            let mut preferences = shortcuts.preferences();
            for _ in 0..2 {
                let restored = ShortcutRegistry::from_preferences(&preferences);
                assert_eq!(restored.action_for(binding), Some(recipient));
                assert_eq!(restored.display_for(donor), "—");
                for alias in donor.default_bindings() {
                    assert_ne!(restored.action_for(alias), Some(donor));
                }
                preferences.reverse();
            }
        }
    }

    #[test]
    fn cancelled_or_different_command_cannot_transfer_a_staged_conflict() {
        let mut shortcuts = ShortcutRegistry::default();
        let mut edit = ShortcutEdit::default();
        let binding = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());
        assert!(
            edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![binding])
                .is_err()
        );
        edit.discard();
        assert!(!edit.overwrite(&mut shortcuts, ShortcutAction::ToggleMute));
        assert!(
            edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![binding])
                .is_err()
        );
        assert!(!edit.overwrite(&mut shortcuts, ShortcutAction::ToggleSettings));
        assert!(shortcuts.preferences().is_empty());
    }

    #[test]
    fn transferred_default_aliases_can_be_restored_from_multiple_owners() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts.transfer_bindings(
            ShortcutAction::ToggleMute,
            vec![ShortcutBinding::new(
                KeyCode::Space,
                ModifiersState::empty(),
            )],
        );
        shortcuts.transfer_bindings(
            ShortcutAction::ToggleSettings,
            vec![ShortcutBinding::new(KeyCode::KeyK, ModifiersState::empty())],
        );
        let defaults = ShortcutAction::PlayPause.default_bindings();
        let notice = shortcuts.transfer_notice(ShortcutAction::PlayPause, &defaults);
        assert!(notice.contains(ShortcutAction::ToggleMute.title()));
        assert!(notice.contains(ShortcutAction::ToggleSettings.title()));
        let mut edit = ShortcutEdit::default();
        assert!(
            edit.stage(&shortcuts, ShortcutAction::PlayPause, defaults.clone())
                .is_err()
        );
        assert!(edit.overwrite(&mut shortcuts, ShortcutAction::PlayPause));
        let restored = ShortcutRegistry::from_preferences(&shortcuts.preferences());
        for binding in defaults {
            assert_eq!(
                restored.action_for(binding),
                Some(ShortcutAction::PlayPause)
            );
        }
        assert_eq!(restored.display_for(ShortcutAction::ToggleMute), "—");
        assert_eq!(restored.display_for(ShortcutAction::ToggleSettings), "—");
    }

    #[test]
    fn swapped_assignments_reload_without_default_order_conflicts() {
        let mut shortcuts = ShortcutRegistry::default();
        let mute = ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty());
        let play = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());
        shortcuts.transfer_bindings(ShortcutAction::PlayPause, vec![mute]);
        shortcuts.rebind(ShortcutAction::ToggleMute, play).unwrap();
        let restored = ShortcutRegistry::from_preferences(&shortcuts.preferences());
        assert_eq!(restored.action_for(mute), Some(ShortcutAction::PlayPause));
        assert_eq!(restored.action_for(play), Some(ShortcutAction::ToggleMute));
    }

    #[test]
    fn restoring_one_command_preserves_other_changes_and_all_default_aliases() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::PlayPause,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyD, ModifiersState::CONTROL),
            )
            .unwrap();
        let mut edit = ShortcutEdit::default();
        edit.stage(
            &shortcuts,
            ShortcutAction::PlayPause,
            ShortcutAction::PlayPause.default_bindings(),
        )
        .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::PlayPause));
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "Ctrl+D");
        for binding in ShortcutAction::PlayPause.default_bindings() {
            assert_eq!(
                shortcuts.action_for(binding),
                Some(ShortcutAction::PlayPause)
            );
        }
        // A default reassigned to another action must not be stolen by reset.
        shortcuts
            .rebind(
                ShortcutAction::PlayPause,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::Space, ModifiersState::empty()),
            )
            .unwrap();
        assert!(
            edit.stage(
                &shortcuts,
                ShortcutAction::PlayPause,
                ShortcutAction::PlayPause.default_bindings()
            )
            .is_err()
        );
    }

    #[test]
    fn restoring_unassigned_default_removes_the_persisted_override() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleSettings,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        let mut edit = ShortcutEdit::default();
        edit.stage(
            &shortcuts,
            ShortcutAction::ToggleSettings,
            ShortcutAction::ToggleSettings.default_bindings(),
        )
        .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::ToggleSettings));
        assert!(shortcuts.preferences().is_empty());
        assert_eq!(
            ShortcutRegistry::from_preferences(&shortcuts.preferences())
                .display_for(ShortcutAction::ToggleSettings),
            "—"
        );
    }

    #[test]
    fn reset_restores_default_shortcuts() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .expect("unused shortcut must be accepted");

        shortcuts.reset();

        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn shortcut_overrides_round_trip_as_physical_keys() {
        let mut shortcuts = ShortcutRegistry::default();
        let replacement = ShortcutBinding::new(
            KeyCode::KeyB,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        );
        shortcuts
            .rebind(ShortcutAction::ToggleMute, replacement)
            .expect("unused shortcut must be accepted");

        let preferences = shortcuts.preferences();
        assert_eq!(preferences.len(), 1);
        let restored = ShortcutRegistry::from_preferences(&preferences);

        assert_eq!(
            restored.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            restored.display_for(ShortcutAction::ToggleMute),
            "Ctrl+Shift+B"
        );
    }

    #[test]
    fn previously_unassigned_commands_can_receive_shortcuts() {
        let mut shortcuts = ShortcutRegistry::default();
        let binding = ShortcutBinding::new(
            KeyCode::Comma,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        );

        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleSettings), "—");
        shortcuts
            .rebind(ShortcutAction::ToggleSettings, binding)
            .expect("unused shortcut must be accepted");

        assert_eq!(
            shortcuts.action_for(binding),
            Some(ShortcutAction::ToggleSettings)
        );
        assert_eq!(
            shortcuts.display_for(ShortcutAction::ToggleSettings),
            "Ctrl+Shift+,"
        );
    }

    #[test]
    fn invalid_shortcut_preferences_keep_safe_defaults() {
        let restored = ShortcutRegistry::from_preferences(&[
            ShortcutPreference {
                action_id: ShortcutAction::ToggleMute as i32,
                key_code: "not-json".to_owned(),
                modifiers: 0,
            },
            ShortcutPreference {
                action_id: ShortcutAction::PlayPause as i32,
                key_code: serde_json::to_string(&KeyCode::KeyB).expect("key code must serialize"),
                modifiers: u8::MAX,
            },
        ]);

        assert_eq!(
            restored.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            restored.action_for(ShortcutBinding::new(
                KeyCode::Space,
                ModifiersState::empty()
            )),
            Some(ShortcutAction::PlayPause)
        );
    }

    #[test]
    fn only_continuous_shortcuts_repeat_while_held() {
        assert!(ShortcutAction::SeekForwardLong.allows_repeat());
        assert!(ShortcutAction::VolumeUp.allows_repeat());
        assert!(!ShortcutAction::ToggleMute.allows_repeat());
        assert!(!ShortcutAction::ToggleCommandPalette.allows_repeat());
    }

    #[test]
    fn creates_open_file_command_for_supported_media_drop() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        let video_path = std::env::temp_dir().join(format!("oxide-drop-{unique}.mkv"));
        let audio_path = std::env::temp_dir().join(format!("oxide-drop-{unique}.flac"));
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
