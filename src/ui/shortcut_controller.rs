//! Owns shortcut capture, command-palette models, persistence, and reset flows.

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use slint::winit_030::{
    EventResult,
    winit::keyboard::{KeyCode, ModifiersState},
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    ipc::Command,
    player::{
        commands::CommandSender,
        shortcuts::{
            ShortcutAction, ShortcutBinding, ShortcutDraftStatus, ShortcutEdit, ShortcutRegistry,
            is_modifier_key,
        },
    },
    settings::AppSettings,
    ui::command_catalog,
};

use super::ui_logic::{AppWindow, CommandEntry, ShortcutCapturePhase};

#[derive(Clone)]
pub(super) struct ShortcutController {
    player_tx: CommandSender,
    settings: Arc<Mutex<AppSettings>>,
    registry: Arc<Mutex<ShortcutRegistry>>,
    edit: Arc<Mutex<ShortcutEdit>>,
    command_items: Rc<VecModel<CommandEntry>>,
}

impl ShortcutController {
    pub(super) fn new(
        app: &AppWindow,
        player_tx: CommandSender,
        settings: Arc<Mutex<AppSettings>>,
    ) -> Self {
        let registry = Arc::new(Mutex::new(
            settings
                .lock()
                .map(|settings| ShortcutRegistry::from_preferences(settings.shortcut_overrides()))
                .unwrap_or_default(),
        ));
        let command_items = Rc::new(VecModel::default());
        app.set_command_items(ModelRc::from(Rc::clone(&command_items)));
        if let Ok(shortcuts) = registry.lock() {
            set_command_palette_items(&command_items, "", &shortcuts);
        }

        let controller = Self {
            player_tx,
            settings,
            registry,
            edit: Arc::new(Mutex::new(ShortcutEdit::default())),
            command_items,
        };
        controller.register_callbacks(app);
        controller
    }

    pub(super) fn handle_capture_or_palette(
        &self,
        app: &AppWindow,
        code: KeyCode,
        modifiers: ModifiersState,
        repeat: bool,
    ) -> Option<EventResult> {
        // The close countdown owns keyboard focus; never record its buttons' keys.
        if app.get_command_palette_visible() && app.get_shortcut_close_pending() {
            return Some(EventResult::Propagate);
        }

        let capture_action = app.get_shortcut_capture_action();
        if capture_action >= 0 && app.get_command_palette_visible() {
            if code == KeyCode::Escape {
                cancel_shortcut_capture(app);
                return Some(EventResult::PreventDefault);
            }
            if repeat {
                return Some(EventResult::PreventDefault);
            }
            // Tab explicitly leaves capture so focused actions can receive Enter/Space.
            if code == KeyCode::Tab {
                if app.get_shortcut_capture_phase() == ShortcutCapturePhase::Recording {
                    app.set_shortcut_capture_phase(ShortcutCapturePhase::Idle);
                    app.set_shortcut_capture_preview("Запись приостановлена".into());
                }
                return Some(EventResult::Propagate);
            }
            if app.get_shortcut_capture_phase() != ShortcutCapturePhase::Recording
                && !app.get_shortcut_capture_focused()
            {
                return Some(EventResult::Propagate);
            }
            if is_modifier_key(code) {
                return Some(EventResult::PreventDefault);
            }
            let Some(action) = ShortcutAction::from_id(capture_action) else {
                cancel_shortcut_capture(app);
                return Some(EventResult::PreventDefault);
            };
            let binding = ShortcutBinding::new(code, modifiers);
            if let Ok(shortcuts) = self.registry.lock()
                && let Ok(mut edit) = self.edit.lock()
            {
                preview_shortcut_edit(app, &mut edit, &shortcuts, action, vec![binding]);
            }
            return Some(EventResult::PreventDefault);
        }

        if app.get_command_palette_visible() && code == KeyCode::Escape {
            app.set_command_palette_visible(false);
            app.set_command_query("".into());
            app.invoke_command_query_changed("".into());
            return Some(EventResult::PreventDefault);
        }

        None
    }

    pub(super) fn dispatch_binding(
        &self,
        app: &AppWindow,
        code: KeyCode,
        modifiers: ModifiersState,
        repeat: bool,
    ) -> Option<EventResult> {
        let binding = ShortcutBinding::new(code, modifiers);
        let action = self
            .registry
            .lock()
            .ok()
            .and_then(|shortcuts| shortcuts.action_for(binding))?;

        if app.get_command_palette_visible() && action != ShortcutAction::ToggleCommandPalette {
            return None;
        }
        if !repeat || action.allows_repeat() {
            dispatch_shortcut(app, action);
        }
        Some(EventResult::PreventDefault)
    }

    fn register_callbacks(&self, app: &AppWindow) {
        {
            let app_weak = app.as_weak();
            app.on_toggle_command_palette(move || {
                if let Some(app) = app_weak.upgrade() {
                    let opening = !app.get_command_palette_visible();
                    app.set_converter_visible(false);
                    cancel_shortcut_capture(&app);
                    app.set_command_query("".into());
                    if !opening {
                        app.invoke_command_query_changed("".into());
                    }
                    app.set_command_palette_visible(opening);
                }
            });
        }

        {
            let controller = self.clone();
            let app_weak = app.as_weak();
            app.on_command_query_changed(move |query| {
                if app_weak.upgrade().is_some() {
                    controller.refresh_items(query.as_str());
                }
            });
        }

        {
            let controller = self.clone();
            let app_weak = app.as_weak();
            app.on_reset_shortcuts(move || {
                if let Some(app) = app_weak.upgrade() {
                    controller.reset_shortcuts(&app);
                }
            });
        }

        {
            let controller = self.clone();
            let app_weak = app.as_weak();
            app.on_reset_settings(move || {
                if let Some(app) = app_weak.upgrade() {
                    controller.reset_settings(&app);
                }
            });
        }

        {
            let controller = self.clone();
            let app_weak = app.as_weak();
            app.on_begin_shortcut_edit(move |action| {
                if let Some(app) = app_weak.upgrade() {
                    controller.begin_edit(&app, action);
                }
            });
        }

        {
            let controller = self.clone();
            let app_weak = app.as_weak();
            app.on_preview_default_shortcut(move || {
                if let Some(app) = app_weak.upgrade() {
                    controller.preview_default(&app);
                }
            });
        }

        {
            let controller = self.clone();
            let app_weak = app.as_weak();
            app.on_save_shortcut_edit(move |overwrite| {
                if let Some(app) = app_weak.upgrade() {
                    controller.save_edit(&app, overwrite);
                }
            });
        }
    }

    fn refresh_items(&self, query: &str) {
        if let Ok(shortcuts) = self.registry.lock() {
            set_command_palette_items(&self.command_items, query, &shortcuts);
        }
    }

    fn reset_shortcuts(&self, app: &AppWindow) {
        if let Ok(mut shortcuts) = self.registry.lock() {
            shortcuts.reset();
            set_command_palette_items(
                &self.command_items,
                app.get_command_query().as_str(),
                &shortcuts,
            );
            persist_shortcuts(app, &self.settings, &shortcuts);
        }
        cancel_shortcut_capture(app);
    }

    fn reset_settings(&self, app: &AppWindow) {
        let defaults = AppSettings::default();
        let Ok(mut shortcuts) = self.registry.lock() else {
            show_error(app, "Не удалось получить доступ к горячим клавишам");
            return;
        };
        let Ok(mut stored_settings) = self.settings.lock() else {
            show_error(app, "Не удалось получить доступ к настройкам");
            return;
        };
        stored_settings.reset_to_defaults();
        if let Err(error) = stored_settings.save_if_dirty() {
            show_error(app, format!("Не удалось сбросить настройки: {error}"));
            return;
        }
        shortcuts.reset();
        set_command_palette_items(
            &self.command_items,
            app.get_command_query().as_str(),
            &shortcuts,
        );
        drop(shortcuts);
        drop(stored_settings);

        if let Ok(mut edit) = self.edit.lock() {
            edit.discard();
        }
        cancel_shortcut_capture(app);

        app.set_theme_mode(defaults.theme().ui_index());
        app.set_accent_mode(defaults.accent().ui_index());
        app.set_volume(defaults.default_volume());
        app.set_speed(defaults.default_speed());
        app.set_folder_loop_enabled(defaults.completion_behavior().repeats_playlist());
        app.set_hardware_decoding_enabled(defaults.hardware_decoding_enabled());
        app.set_settings_reset_done(true);
        app.set_reset_settings_prompt_visible(false);
        app.set_notice_message("Настройки восстановлены".into());
        app.set_notice_visible(true);

        let _ = self
            .player_tx
            .send(Command::SetDarkTheme(app.get_dark_theme()));
        let _ = self.player_tx.send(Command::SetVolume(
            f64::from(defaults.default_volume()) * 100.0,
        ));
        let _ = self
            .player_tx
            .send(Command::SetSpeed(f64::from(defaults.default_speed())));
        let _ = self.player_tx.send(Command::SetFolderLoop(
            defaults.completion_behavior().repeats_playlist(),
        ));
        let _ = self.player_tx.send(Command::SetHardwareDecoding(
            defaults.hardware_decoding_enabled(),
        ));
        app.window().request_redraw();

        let notice_app = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2600), move || {
            if let Some(app) = notice_app.upgrade() {
                app.set_notice_visible(false);
            }
        });
    }

    fn begin_edit(&self, app: &AppWindow, action: i32) {
        if ShortcutAction::from_id(action).is_some()
            && let Ok(mut edit) = self.edit.lock()
        {
            edit.discard();
            app.set_shortcut_saved_action(-1);
            clear_shortcut_candidate(app);
            app.set_shortcut_capture_phase(ShortcutCapturePhase::Recording);
            app.set_shortcut_capture_action(action);
        }
    }

    fn preview_default(&self, app: &AppWindow) {
        if let Some(action) = ShortcutAction::from_id(app.get_shortcut_capture_action())
            && let Ok(shortcuts) = self.registry.lock()
            && let Ok(mut edit) = self.edit.lock()
        {
            preview_shortcut_edit(
                app,
                &mut edit,
                &shortcuts,
                action,
                action.default_bindings(),
            );
        }
    }

    fn save_edit(&self, app: &AppWindow, overwrite: bool) {
        if !app.get_command_palette_visible()
            || (app.get_shortcut_capture_phase() != ShortcutCapturePhase::Valid
                && !(overwrite
                    && app.get_shortcut_capture_phase() == ShortcutCapturePhase::Conflict))
        {
            return;
        }
        let Some(action) = ShortcutAction::from_id(app.get_shortcut_capture_action()) else {
            return;
        };
        let Ok(mut shortcuts) = self.registry.lock() else {
            return;
        };
        let Ok(mut edit) = self.edit.lock() else {
            return;
        };
        let saved = if overwrite {
            edit.overwrite(&mut shortcuts, action)
        } else {
            edit.save(&mut shortcuts, action)
        };
        if !saved {
            return;
        }

        set_command_palette_items(
            &self.command_items,
            app.get_command_query().as_str(),
            &shortcuts,
        );
        persist_shortcuts(app, &self.settings, &shortcuts);
        cancel_shortcut_capture(app);
        app.set_shortcut_saved_action(action as i32);

        let feedback_app = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1600), move || {
            if let Some(app) = feedback_app.upgrade()
                && app.get_shortcut_saved_action() == action as i32
            {
                app.set_shortcut_saved_action(-1);
            }
        });
    }
}

fn preview_shortcut_edit(
    app: &AppWindow,
    edit: &mut ShortcutEdit,
    registry: &ShortcutRegistry,
    action: ShortcutAction,
    bindings: Vec<ShortcutBinding>,
) {
    let transfer_notice = registry.transfer_notice(action, &bindings);
    let draft = edit.stage_state(registry, action, bindings);
    app.set_shortcut_capture_preview(draft.label.into());
    app.set_shortcut_capture_dirty(draft.dirty);
    match draft.status {
        ShortcutDraftStatus::Valid => {
            app.set_shortcut_capture_phase(ShortcutCapturePhase::Valid);
            app.set_shortcut_capture_error("".into());
        }
        ShortcutDraftStatus::Conflict(conflict) => {
            app.set_shortcut_capture_phase(ShortcutCapturePhase::Conflict);
            app.set_shortcut_capture_error(
                format!(
                    "{} занято командой «{}».\n{transfer_notice}",
                    conflict.binding.display(),
                    conflict.action.title()
                )
                .into(),
            );
        }
    }
}

fn clear_shortcut_candidate(app: &AppWindow) {
    app.set_shortcut_capture_phase(ShortcutCapturePhase::Idle);
    app.set_shortcut_capture_dirty(false);
    app.set_shortcut_capture_preview("".into());
    app.set_shortcut_capture_error("".into());
}

fn cancel_shortcut_capture(app: &AppWindow) {
    app.set_shortcut_capture_action(-1);
    app.set_shortcut_capture_focused(false);
    clear_shortcut_candidate(app);
}

fn dispatch_shortcut(app: &AppWindow, action: ShortcutAction) {
    app.invoke_reveal_controls();
    match action {
        ShortcutAction::PlayPause => app.invoke_play_pause(),
        ShortcutAction::Stop => app.invoke_stop(),
        ShortcutAction::Previous => app.invoke_previous_video(),
        ShortcutAction::Next => app.invoke_next_video(),
        ShortcutAction::SeekBackwardShort => app.invoke_seek_relative(-5.0),
        ShortcutAction::SeekForwardShort => app.invoke_seek_relative(5.0),
        ShortcutAction::SeekBackwardLong => app.invoke_seek_relative(-10.0),
        ShortcutAction::SeekForwardLong => app.invoke_seek_relative(10.0),
        ShortcutAction::FrameBackward => app.invoke_step_frame(-1),
        ShortcutAction::FrameForward => app.invoke_step_frame(1),
        ShortcutAction::VolumeDown => app.invoke_adjust_volume(-5.0),
        ShortcutAction::VolumeUp => app.invoke_adjust_volume(5.0),
        ShortcutAction::ToggleMute => app.invoke_toggle_mute(),
        ShortcutAction::ToggleLoop => app.invoke_toggle_loop(),
        ShortcutAction::ToggleFullscreen => app.invoke_toggle_fullscreen(),
        ShortcutAction::TogglePictureInPicture => app.invoke_toggle_picture_in_picture(),
        ShortcutAction::ToggleInfo => app.invoke_toggle_info(),
        ShortcutAction::TogglePlaylist => app.invoke_toggle_playlist(),
        ShortcutAction::ToggleCommandPalette => app.invoke_toggle_command_palette(),
        ShortcutAction::ToggleSettings => app.invoke_toggle_settings(),
        ShortcutAction::OpenConverter => app.invoke_open_converter(),
        ShortcutAction::OpenFile => app.invoke_open_file(),
        ShortcutAction::OpenFolder => app.invoke_open_folder(),
    }
}

pub(super) fn command_palette_items_with_registry(
    query: &str,
    shortcuts: &ShortcutRegistry,
) -> Vec<CommandEntry> {
    command_catalog::search(query)
        .into_iter()
        .map(|command| CommandEntry {
            action: command.action,
            title: command.title.into(),
            description: command.description.into(),
            shortcut: ShortcutAction::from_palette_action(command.action)
                .map(|action| shortcuts.display_for(action))
                .unwrap_or_else(|| command.shortcut.to_string())
                .into(),
            requires_media: command.requires_media,
            shortcut_editable: ShortcutAction::from_palette_action(command.action).is_some(),
            default_shortcut: ShortcutAction::from_palette_action(command.action)
                .map(|action| {
                    let defaults = action.default_bindings();
                    if defaults.is_empty() {
                        "не назначено".to_string()
                    } else {
                        defaults
                            .iter()
                            .map(|binding| binding.display())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    }
                })
                .unwrap_or_default()
                .into(),
        })
        .collect()
}

#[cfg(test)]
pub(super) fn command_palette_items(query: &str) -> Vec<CommandEntry> {
    command_palette_items_with_registry(query, &ShortcutRegistry::default())
}

fn set_command_palette_items(
    model: &VecModel<CommandEntry>,
    query: &str,
    shortcuts: &ShortcutRegistry,
) {
    let items = command_palette_items_with_registry(query, shortcuts);
    let can_update_in_place = model.row_count() == items.len()
        && items.iter().enumerate().all(|(index, item)| {
            model
                .row_data(index)
                .is_some_and(|current| current.action == item.action)
        });

    if can_update_in_place {
        // Preserve repeater instances so an expanded shortcut row can finish
        // its close animation after the saved label changes.
        for (index, item) in items.into_iter().enumerate() {
            model.set_row_data(index, item);
        }
    } else {
        model.set_vec(items);
    }
}

fn persist_shortcuts(
    app: &AppWindow,
    settings: &Arc<Mutex<AppSettings>>,
    shortcuts: &ShortcutRegistry,
) {
    if let Ok(mut settings) = settings.lock() {
        settings.set_shortcut_overrides(shortcuts.preferences());
        if let Err(error) = settings.save_if_dirty() {
            show_error(
                app,
                format!("Не удалось сохранить горячие клавиши: {error}"),
            );
        }
    }
}

fn show_error(app: &AppWindow, message: impl Into<slint::SharedString>) {
    app.set_error_message(message.into());
    app.set_error_visible(true);
}
