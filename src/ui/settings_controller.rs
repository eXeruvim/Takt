use std::sync::{Arc, Mutex};

use slint::ComponentHandle;

use crate::{
    ipc::Command,
    player::commands::CommandSender,
    settings::{AccentPreference, AppSettings, CompletionBehavior, ThemePreference},
};

use super::ui_logic::AppWindow;

pub(super) fn register_preference_callbacks(
    app: &AppWindow,
    tx: &CommandSender,
    settings: &Arc<Mutex<AppSettings>>,
) {
    {
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_choose_accent(move |index| {
            let Some(accent) = AccentPreference::from_ui_index(index) else {
                return;
            };
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            app.set_accent_mode(accent.ui_index());
            app.window().request_redraw();
            update_settings(
                &app,
                &settings,
                "Не удалось сохранить акцентный цвет",
                |settings| settings.set_accent(accent),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_persist_playback_defaults(move |volume, speed| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            update_settings(
                &app,
                &settings,
                "Не удалось сохранить настройки воспроизведения",
                |settings| {
                    settings.set_default_volume(volume);
                    settings.set_default_speed(speed);
                },
            );
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_set_hardware_decoding(move |enabled| {
            let _ = tx.send(Command::SetHardwareDecoding(enabled));
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            update_settings(
                &app,
                &settings,
                "Не удалось сохранить настройку аппаратного декодирования",
                |settings| settings.set_hardware_decoding_enabled(enabled),
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_persist_muted(move |muted| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            update_settings(
                &app,
                &settings,
                "Не удалось сохранить состояние звука",
                |settings| settings.set_muted(muted),
            );
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_choose_theme(move |index| {
            let Some(theme) = ThemePreference::from_ui_index(index) else {
                return;
            };
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            app.set_theme_mode(theme.ui_index());
            let _ = tx.send(Command::SetDarkTheme(app.get_dark_theme()));
            app.window().request_redraw();
            update_settings(
                &app,
                &settings,
                "Не удалось сохранить тему оформления",
                |settings| settings.set_theme(theme),
            );
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_set_folder_loop(move |enabled| {
            let _ = tx.send(Command::SetFolderLoop(enabled));
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            update_settings(
                &app,
                &settings,
                "Не удалось сохранить поведение после завершения",
                |settings| {
                    settings.set_completion_behavior(if enabled {
                        CompletionBehavior::RepeatPlaylist
                    } else {
                        CompletionBehavior::Stop
                    });
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let settings = settings.clone();
        app.on_persist_shuffle(move |enabled| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            update_settings(
                &app,
                &settings,
                "Не удалось сохранить случайный порядок",
                |settings| settings.set_shuffle_enabled(enabled),
            );
        });
    }
}

fn update_settings(
    app: &AppWindow,
    settings: &Arc<Mutex<AppSettings>>,
    error_context: &str,
    update: impl FnOnce(&mut AppSettings),
) {
    let Ok(mut settings) = settings.lock() else {
        return;
    };
    update(&mut settings);
    if let Err(error) = settings.save_if_dirty() {
        app.set_error_message(format!("{error_context}: {error}").into());
        app.set_error_visible(true);
    }
}
