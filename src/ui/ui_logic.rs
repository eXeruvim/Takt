#![deny(clippy::undocumented_unsafe_blocks)]

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};

use crate::converter::{ConverterCommand, ConverterEvent};
use crate::ipc::{Command, Event};
use crate::player::commands::{CommandSender, SendError as PlayerCommandError};
use crate::render::mpv_opengl::MpvRenderSource;
use crate::settings::AppSettings;
use crate::ui::player_event_controller::PlayerEventController;
use crate::ui::shortcut_controller::ShortcutController;
#[cfg(target_os = "windows")]
use crate::ui::system_notification;
#[cfg(target_os = "windows")]
use crate::ui::video_output::VideoOutputController;
#[cfg(target_os = "windows")]
use crate::ui::window_interaction::{WindowDragState, native_window_handle};
#[cfg(target_os = "windows")]
use crate::ui::window_modes::WindowModeController;
#[cfg(target_os = "windows")]
use crate::ui::window_placement::restore_window_placement;
#[cfg(target_os = "windows")]
use crate::ui::windows_theme::prefers_dark_apps as windows_prefers_dark_apps;
slint::include_modules!();

pub struct UiResources {
    pub converter_tx: Sender<ConverterCommand>,
    pub converter_rx: Receiver<ConverterEvent>,
    pub converter_control: Arc<crate::converter::ConverterControl>,
    pub update_probe: Option<std::path::PathBuf>,
}

pub fn run(
    tx: CommandSender,
    rx: Receiver<Event>,
    render_source_rx: Receiver<MpvRenderSource>,
    command_errors: Receiver<PlayerCommandError>,
    resources: UiResources,
) -> Result<()> {
    let UiResources {
        converter_tx,
        converter_rx,
        converter_control,
        update_probe,
    } = resources;
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
        app.set_muted(settings.muted());
        app.set_speed(settings.default_speed());
        app.set_folder_loop_enabled(settings.completion_behavior().repeats_playlist());
        app.set_shuffle_enabled(settings.shuffle_enabled());
        app.set_hardware_decoding_enabled(settings.hardware_decoding_enabled());
        let _ = tx.send(Command::SetVolume(
            f64::from(settings.default_volume()) * 100.0,
        ));
        let _ = tx.send(Command::SetMuted(settings.muted()));
        let _ = tx.send(Command::SetSpeed(f64::from(settings.default_speed())));
        let _ = tx.send(Command::SetFolderLoop(
            settings.completion_behavior().repeats_playlist(),
        ));
        let _ = tx.send(Command::SetShuffle(settings.shuffle_enabled()));
        let _ = tx.send(Command::SetHardwareDecoding(
            settings.hardware_decoding_enabled(),
        ));
    }
    if let Some(message) = take_update_diagnostic() {
        app.set_error_message(format!("Обновление: {message}").into());
        app.set_error_visible(true);
    }
    super::conversion_controller::apply_presets(&app, false, "");
    let shortcuts = ShortcutController::new(&app, tx.clone(), settings.clone());
    let dialog_active = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let video_output = VideoOutputController::default();
    #[cfg(target_os = "windows")]
    let video_output_events = video_output.event_handle();
    #[cfg(target_os = "windows")]
    let window_modes = WindowModeController::default();
    #[cfg(target_os = "windows")]
    let window_drag_state = Arc::new(Mutex::new(WindowDragState::default()));
    let player_events = PlayerEventController::new(
        #[cfg(target_os = "windows")]
        video_output_events.clone(),
        #[cfg(target_os = "windows")]
        tx.clone(),
    );
    let current_duration = player_events.duration_handle();

    super::window_event_controller::register_callbacks(
        &app,
        &tx,
        &settings,
        &shortcuts,
        #[cfg(target_os = "windows")]
        &window_drag_state,
    );

    super::settings_controller::register_preference_callbacks(&app, &tx, &settings);
    let _update_controller = super::update_controller::install(&app, settings.clone())?;
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

    super::drawer_controller::register_callbacks(&app);

    super::conversion_controller::register_callbacks(
        &app,
        converter_tx,
        converter_control,
        dialog_active.clone(),
    );

    #[cfg(target_os = "windows")]
    let native_init_error_rx = video_output.register_rendering(&app, tx.clone())?;

    #[cfg(target_os = "windows")]
    let render_source_bridge = video_output_events.spawn_source_bridge(&app, render_source_rx)?;
    #[cfg(not(target_os = "windows"))]
    drop(render_source_rx);

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
    if let Some(path) = update_probe
        && let Err(error) = write_update_probe(&path)
    {
        eprintln!("Не удалось подтвердить первый запуск обновления: {error}");
    }
    let dialog_fault = crate::benchmark_diagnostics::fault_requested("dialog-open");
    let dialog_open_timer = dialog_fault.then(slint::Timer::default);
    let dialog_shutdown_timer = dialog_fault.then(slint::Timer::default);
    if let Some(timer) = dialog_open_timer.as_ref() {
        let app_weak = app.as_weak();
        timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(350),
            move || {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                eprintln!("OXIDE_DIAGNOSTIC_FAULT:dialog-open");
                app.invoke_open_file();
            },
        );
    }
    if let Some(timer) = dialog_shutdown_timer.as_ref() {
        timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(1_500),
            || {
                eprintln!("OXIDE_DIAGNOSTIC_FAULT:dialog-shutdown");
                let _ = slint::quit_event_loop();
            },
        );
    }
    let _ui_cycle_driver = super::benchmark_cycle_controller::install(&app);
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
    drop(dialog_open_timer);
    drop(dialog_shutdown_timer);
    let hide_result = app.hide();
    drop(player_bridge);
    #[cfg(target_os = "windows")]
    drop(render_source_bridge);
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

fn write_update_probe(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write as _;
    let expected = std::env::temp_dir().join("Oxide").join("update-probes");
    if path.parent() != Some(expected.as_path())
        || path.extension().and_then(|value| value.to_str()) != Some("ready")
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "маркер обновления находится вне управляемого каталога",
        ));
    }
    std::fs::create_dir_all(&expected)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    writeln!(file, "{}", env!("CARGO_PKG_VERSION"))?;
    file.sync_all()
}

fn take_update_diagnostic() -> Option<String> {
    const MAX_DIAGNOSTIC_BYTES: u64 = 16 * 1024;
    let path = std::env::temp_dir()
        .join("Oxide")
        .join("last-update-error.txt");
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_DIAGNOSTIC_BYTES {
        let _ = std::fs::remove_file(path);
        return Some("диагностический отчёт повреждён".to_owned());
    }
    let message = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(path);
    message.map(|message| message.trim().to_owned())
}

#[cfg(test)]
mod tests {
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
}
