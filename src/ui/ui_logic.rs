#[cfg(target_os = "windows")]
use std::time::Duration;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};

use crate::ipc::{Command, Event};
use crate::player::playlist::{
    is_media_file, AUDIO_EXTENSIONS, MEDIA_EXTENSIONS, VIDEO_EXTENSIONS,
};
#[cfg(target_os = "windows")]
use crate::render::mpv_render::VideoHost;
use slint::winit_030::{
    winit::{
        event::WindowEvent,
        keyboard::{KeyCode, ModifiersState, PhysicalKey},
    },
    EventResult, WinitWindowAccessor,
};
use slint::{ModelRc, SharedString, VecModel};

slint::include_modules!();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutAction {
    PlayPause,
    Stop,
    Previous,
    Next,
    SeekBackwardShort,
    SeekForwardShort,
    SeekBackwardLong,
    SeekForwardLong,
    VolumeDown,
    VolumeUp,
    ToggleMute,
    ToggleLoop,
    ToggleFullscreen,
    ToggleInfo,
    TogglePlaylist,
    ToggleShortcuts,
    ToggleCommandPalette,
    OpenFile,
    OpenFolder,
}

impl ShortcutAction {
    fn allows_repeat(self) -> bool {
        matches!(
            self,
            Self::SeekBackwardShort
                | Self::SeekForwardShort
                | Self::SeekBackwardLong
                | Self::SeekForwardLong
                | Self::VolumeDown
                | Self::VolumeUp
        )
    }
}

fn shortcut_action(code: KeyCode, modifiers: ModifiersState) -> Option<ShortcutAction> {
    use ShortcutAction as Action;

    match (code, modifiers) {
        (KeyCode::KeyK, ModifiersState::CONTROL) => Some(Action::ToggleCommandPalette),
        (KeyCode::KeyO, mods) if mods == ModifiersState::CONTROL | ModifiersState::SHIFT => {
            Some(Action::OpenFolder)
        }
        (KeyCode::KeyO, ModifiersState::CONTROL) => Some(Action::OpenFile),
        (KeyCode::KeyL, ModifiersState::CONTROL) => Some(Action::TogglePlaylist),
        (KeyCode::Space | KeyCode::KeyK, mods) if mods.is_empty() => Some(Action::PlayPause),
        (KeyCode::KeyS, mods) if mods.is_empty() => Some(Action::Stop),
        (KeyCode::KeyP, mods) if mods.is_empty() => Some(Action::Previous),
        (KeyCode::KeyN, mods) if mods.is_empty() => Some(Action::Next),
        (KeyCode::ArrowLeft, mods) if mods.is_empty() => Some(Action::SeekBackwardShort),
        (KeyCode::ArrowRight, mods) if mods.is_empty() => Some(Action::SeekForwardShort),
        (KeyCode::KeyJ, mods) if mods.is_empty() => Some(Action::SeekBackwardLong),
        (KeyCode::KeyL, mods) if mods.is_empty() => Some(Action::SeekForwardLong),
        (KeyCode::ArrowDown, mods) if mods.is_empty() => Some(Action::VolumeDown),
        (KeyCode::ArrowUp, mods) if mods.is_empty() => Some(Action::VolumeUp),
        (KeyCode::KeyM, mods) if mods.is_empty() => Some(Action::ToggleMute),
        (KeyCode::KeyR, mods) if mods.is_empty() => Some(Action::ToggleLoop),
        (KeyCode::KeyF, mods) if mods.is_empty() => Some(Action::ToggleFullscreen),
        (KeyCode::KeyI, mods) if mods.is_empty() => Some(Action::ToggleInfo),
        (KeyCode::KeyH, mods) if mods.is_empty() => Some(Action::ToggleShortcuts),
        (KeyCode::KeyO, mods) if mods.is_empty() => Some(Action::OpenFile),
        _ => None,
    }
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
        ShortcutAction::VolumeDown => app.invoke_adjust_volume(-5.0),
        ShortcutAction::VolumeUp => app.invoke_adjust_volume(5.0),
        ShortcutAction::ToggleMute => app.invoke_toggle_mute(),
        ShortcutAction::ToggleLoop => app.invoke_toggle_loop(),
        ShortcutAction::ToggleFullscreen => app.invoke_toggle_fullscreen(),
        ShortcutAction::ToggleInfo => app.invoke_toggle_info(),
        ShortcutAction::TogglePlaylist => app.invoke_toggle_playlist(),
        ShortcutAction::ToggleShortcuts => app.invoke_toggle_shortcuts(),
        ShortcutAction::ToggleCommandPalette => app.invoke_toggle_command_palette(),
        ShortcutAction::OpenFile => app.invoke_open_file(),
        ShortcutAction::OpenFolder => app.invoke_open_folder(),
    }
}

struct PaletteCommandSpec {
    action: i32,
    title: &'static str,
    description: &'static str,
    shortcut: &'static str,
    keywords: &'static str,
    requires_media: bool,
}

const PALETTE_COMMANDS: &[PaletteCommandSpec] = &[
    PaletteCommandSpec {
        action: 0,
        title: "Открыть файл",
        description: "Выбрать видео или аудиофайл",
        shortcut: "Ctrl O",
        keywords: "open media video audio открыть медиа видео аудио",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 1,
        title: "Открыть папку",
        description: "Собрать плейлист из папки",
        shortcut: "Ctrl Shift O",
        keywords: "open folder directory playlist открыть каталог плейлист",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 2,
        title: "Пауза / воспроизведение",
        description: "Переключить состояние плеера",
        shortcut: "Space",
        keywords: "play pause start stop играть пауза воспроизведение",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 3,
        title: "Предыдущий файл",
        description: "Перейти назад по плейлисту",
        shortcut: "P",
        keywords: "previous back playlist предыдущий назад",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 4,
        title: "Следующий файл",
        description: "Перейти вперёд по плейлисту",
        shortcut: "N",
        keywords: "next forward playlist следующий вперед вперёд",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 13,
        title: "Назад на 10 секунд",
        description: "Перемотать текущий файл назад",
        shortcut: "J",
        keywords: "seek rewind back перемотка назад",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 14,
        title: "Вперёд на 10 секунд",
        description: "Перемотать текущий файл вперёд",
        shortcut: "L",
        keywords: "seek forward перемотка вперед вперёд",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 5,
        title: "Выключить / включить звук",
        description: "Переключить mute",
        shortcut: "M",
        keywords: "mute unmute sound audio звук без звука",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 15,
        title: "Уменьшить громкость",
        description: "Убавить громкость на 5%",
        shortcut: "↓",
        keywords: "volume down sound громкость тише убавить",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 16,
        title: "Увеличить громкость",
        description: "Прибавить громкость на 5%",
        shortcut: "↑",
        keywords: "volume up sound громкость громче прибавить",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 6,
        title: "Повтор файла",
        description: "Включить или выключить повтор",
        shortcut: "R",
        keywords: "repeat loop повтор цикл",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 7,
        title: "Показать плейлист",
        description: "Открыть или закрыть очередь файлов",
        shortcut: "Ctrl L",
        keywords: "playlist queue список очередь файлы",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 8,
        title: "Свойства файла",
        description: "Показать информацию о медиа",
        shortcut: "I",
        keywords: "info properties metadata свойства информация метаданные",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 9,
        title: "Настройки воспроизведения",
        description: "Скорость, повтор папки и перемешивание",
        shortcut: "—",
        keywords: "settings speed shuffle настройки скорость перемешивание",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 17,
        title: "Скорость: 0.5×",
        description: "Установить половинную скорость",
        shortcut: "—",
        keywords: "speed slow скорость медленно половина",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 18,
        title: "Скорость: 1×",
        description: "Вернуть обычную скорость",
        shortcut: "—",
        keywords: "speed normal reset скорость обычно сброс",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 19,
        title: "Скорость: 1.25×",
        description: "Установить скорость 1.25×",
        shortcut: "—",
        keywords: "speed скорость",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 20,
        title: "Скорость: 1.5×",
        description: "Установить скорость 1.5×",
        shortcut: "—",
        keywords: "speed fast скорость быстро",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 21,
        title: "Скорость: 2×",
        description: "Установить двойную скорость",
        shortcut: "—",
        keywords: "speed fast скорость быстро двойная",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 10,
        title: "Горячие клавиши",
        description: "Показать справку по управлению",
        shortcut: "H",
        keywords: "help shortcuts keyboard помощь клавиши управление",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 11,
        title: "Полноэкранный режим",
        description: "Переключить fullscreen",
        shortcut: "F",
        keywords: "fullscreen full screen полный экран",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 12,
        title: "Остановить воспроизведение",
        description: "Закрыть текущий файл",
        shortcut: "S",
        keywords: "stop close остановить закрыть",
        requires_media: true,
    },
];

fn command_palette_items(query: &str) -> Vec<CommandEntry> {
    let words = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    PALETTE_COMMANDS
        .iter()
        .filter(|command| {
            let searchable = format!(
                "{} {} {}",
                command.title, command.description, command.keywords
            )
            .to_lowercase();
            words.iter().all(|word| searchable.contains(word))
        })
        .map(|command| CommandEntry {
            action: command.action,
            title: command.title.into(),
            description: command.description.into(),
            shortcut: command.shortcut.into(),
            requires_media: command.requires_media,
        })
        .collect()
}

fn set_command_palette_items(app: &AppWindow, query: &str) {
    app.set_command_items(ModelRc::new(VecModel::from(command_palette_items(query))));
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn is_audio_only_media(width: u32, height: u32, video_codec: &str, audio_codec: &str) -> bool {
    width == 0 && height == 0 && video_codec.is_empty() && !audio_codec.is_empty()
}

fn command_for_dropped_path(path: &Path) -> Result<Command, String> {
    if path.is_dir() {
        return Ok(Command::OpenFolder(path.to_string_lossy().into_owned()));
    }
    if path.is_file() && is_media_file(path) {
        return Ok(Command::OpenFile(path.to_string_lossy().into_owned()));
    }

    Err(format!(
        "Перетащенный объект не является поддерживаемым медиафайлом или папкой: {}",
        path.display()
    ))
}

#[cfg(target_os = "windows")]
fn sync_video_host(host: &mut VideoHost, app: &AppWindow) -> Result<()> {
    let video_visible = app.get_media_loaded()
        && !app.get_audio_media()
        && !app.get_shortcut_overlay_visible()
        && !app.get_command_palette_visible()
        && !app.get_error_visible()
        && !app.get_drop_target_visible();
    host.set_visible(video_visible);

    let left_inset = if app.get_info_drawer_reserved() || app.get_playlist_drawer_reserved() {
        368.0
    } else {
        16.0
    };
    let right_inset = if app.get_settings_drawer_reserved() {
        388.0
    } else {
        16.0
    };
    host.resize(
        app.window(),
        left_inset,
        right_inset,
        app.get_video_top_inset(),
        app.get_video_bottom_inset(),
    )
}

pub fn run(tx: Sender<Command>, rx: Receiver<Event>) -> Result<()> {
    let app = AppWindow::new()?;
    set_command_palette_items(&app, "");
    let current_duration = Arc::new(Mutex::new(0.0));
    let dialog_active = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let video_host = Arc::new(Mutex::new(None::<VideoHost>));
    #[cfg(target_os = "windows")]
    let video_resize_timer = slint::Timer::default();

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
        let mut modifiers = ModifiersState::empty();
        let mut last_cursor_position = None::<(f64, f64)>;

        app.window().on_winit_window_event(move |_, event| {
            let Some(app) = app_weak.upgrade() else {
                return EventResult::Propagate;
            };

            match event {
                WindowEvent::CursorMoved { position, .. } => {
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
                }
                WindowEvent::CursorLeft { .. } => {
                    app.set_controls_pointer_over(false);
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                    let PhysicalKey::Code(code) = event.physical_key else {
                        return EventResult::Propagate;
                    };
                    let Some(action) = shortcut_action(code, modifiers) else {
                        return EventResult::Propagate;
                    };

                    if app.get_command_palette_visible()
                        && action != ShortcutAction::ToggleCommandPalette
                    {
                        return EventResult::Propagate;
                    }
                    if app.get_shortcut_overlay_visible()
                        && !matches!(
                            action,
                            ShortcutAction::ToggleShortcuts | ShortcutAction::ToggleCommandPalette
                        )
                    {
                        return EventResult::PreventDefault;
                    }
                    if !event.repeat || action.allows_repeat() {
                        dispatch_shortcut(&app, action);
                    }
                    return EventResult::PreventDefault;
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

    {
        let tx = tx.clone();
        let dialog_active = dialog_active.clone();

        app.on_open_file(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = dialog_active.clone();
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("Медиа", MEDIA_EXTENSIONS)
                    .add_filter("Видео", VIDEO_EXTENSIONS)
                    .add_filter("Аудио", AUDIO_EXTENSIONS)
                    .pick_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::OpenFile(path.to_string_lossy().into_owned()));
                }
            });
        });
    }

    {
        let tx = tx.clone();
        let dialog_active = dialog_active.clone();

        app.on_open_folder(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = dialog_active.clone();
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new().pick_folder();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::OpenFolder(path.to_string_lossy().into_owned()));
                }
            });
        });
    }

    {
        let tx = tx.clone();

        app.on_play_pause(move || {
            let _ = tx.send(Command::TogglePlay);
        });
    }

    {
        let tx = tx.clone();

        app.on_stop(move || {
            let _ = tx.send(Command::Stop);
        });
    }

    {
        let tx = tx.clone();
        let current_duration = current_duration.clone();

        app.on_seek(move |value| {
            let duration = current_duration
                .lock()
                .map(|duration| *duration)
                .unwrap_or_default();
            let seek_to = value as f64 * duration;
            let _ = tx.send(Command::Seek(seek_to));
        });
    }

    {
        let tx = tx.clone();

        app.on_set_volume(move |value| {
            let _ = tx.send(Command::SetVolume(value as f64 * 100.0));
        });
    }

    {
        let tx = tx.clone();

        app.on_set_speed(move |value| {
            let _ = tx.send(Command::SetSpeed(value as f64));
        });
    }

    {
        let tx = tx.clone();

        app.on_next_video(move || {
            let _ = tx.send(Command::NextVideo);
        });
    }

    {
        let tx = tx.clone();

        app.on_previous_video(move || {
            let _ = tx.send(Command::PreviousVideo);
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_loop(move || {
            let _ = tx.send(Command::ToggleLoop);
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_folder_loop(move || {
            let _ = tx.send(Command::ToggleFolderLoop);
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_shuffle(move || {
            let _ = tx.send(Command::ToggleShuffle);
        });
    }

    {
        let tx = tx.clone();

        app.on_request_media_info(move || {
            let _ = tx.send(Command::RequestMediaInfo);
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().set_fullscreen(!app.window().is_fullscreen());
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let panel_video_host = video_host.clone();

        app.on_toggle_info(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_info_drawer_visible();
                if opening {
                    app.set_info_drawer_reserved(true);
                    app.set_playlist_drawer_visible(false);
                    app.set_playlist_drawer_reserved(false);
                    if let Ok(mut video_host) = panel_video_host.lock() {
                        if let Some(host) = video_host.as_mut() {
                            let _ = sync_video_host(host, &app);
                        }
                    }
                    app.set_info_drawer_visible(true);
                } else {
                    app.set_info_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(200), move || {
                        if let Some(app) = delayed_app.upgrade() {
                            if !app.get_info_drawer_visible() {
                                app.set_info_drawer_reserved(false);
                            }
                        }
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_toggle_info(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_info_drawer_visible();
                if opening {
                    app.set_playlist_drawer_visible(false);
                }
                app.set_info_drawer_visible(opening);
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let panel_video_host = video_host.clone();

        app.on_toggle_playlist(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_playlist_drawer_visible();
                if opening {
                    app.set_playlist_drawer_reserved(true);
                    app.set_info_drawer_visible(false);
                    app.set_info_drawer_reserved(false);
                    if let Ok(mut video_host) = panel_video_host.lock() {
                        if let Some(host) = video_host.as_mut() {
                            let _ = sync_video_host(host, &app);
                        }
                    }
                    app.set_playlist_drawer_visible(true);
                } else {
                    app.set_playlist_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(200), move || {
                        if let Some(app) = delayed_app.upgrade() {
                            if !app.get_playlist_drawer_visible() {
                                app.set_playlist_drawer_reserved(false);
                            }
                        }
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_toggle_playlist(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_playlist_drawer_visible();
                if opening {
                    app.set_info_drawer_visible(false);
                }
                app.set_playlist_drawer_visible(opening);
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let panel_video_host = video_host.clone();

        app.on_toggle_settings(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_settings_drawer_visible();
                if opening {
                    app.set_settings_drawer_reserved(true);
                    if let Ok(mut video_host) = panel_video_host.lock() {
                        if let Some(host) = video_host.as_mut() {
                            let _ = sync_video_host(host, &app);
                        }
                    }
                    app.set_settings_drawer_visible(true);
                } else {
                    app.set_settings_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(200), move || {
                        if let Some(app) = delayed_app.upgrade() {
                            if !app.get_settings_drawer_visible() {
                                app.set_settings_drawer_reserved(false);
                            }
                        }
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_toggle_settings(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_settings_drawer_visible(!app.get_settings_drawer_visible());
            }
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_toggle_shortcuts(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_command_palette_visible(false);
                app.set_shortcut_overlay_visible(!app.get_shortcut_overlay_visible());
            }
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_toggle_command_palette(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_command_palette_visible();
                app.set_shortcut_overlay_visible(false);
                app.set_command_query("".into());
                set_command_palette_items(&app, "");
                app.set_command_palette_visible(opening);
            }
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_command_query_changed(move |query| {
            if let Some(app) = app_weak.upgrade() {
                set_command_palette_items(&app, query.as_str());
            }
        });
    }

    {
        let tx = tx.clone();

        app.on_seek_relative(move |value| {
            let _ = tx.send(Command::SeekRelative(value as f64));
        });
    }

    {
        let tx = tx.clone();

        app.on_adjust_volume(move |value| {
            let _ = tx.send(Command::AdjustVolume(value as f64));
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_mute(move || {
            let _ = tx.send(Command::ToggleMute);
        });
    }

    {
        let tx = tx.clone();

        app.on_play_next(move || {
            let _ = tx.send(Command::NextVideo);
        });
    }

    {
        let tx = tx.clone();

        app.on_play_playlist_index(move |index| {
            if let Ok(index) = usize::try_from(index) {
                let _ = tx.send(Command::PlayIndex(index));
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let tx_handle = tx.clone();
        let notifier_video_host = video_host.clone();

        app.window()
            .set_rendering_notifier(move |state, _| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let Ok(mut video_host) = notifier_video_host.lock() else {
                    return;
                };

                match state {
                    slint::RenderingState::RenderingSetup => {
                        match VideoHost::create(app.window()) {
                            Ok(mut host) => {
                                if let Err(error) = sync_video_host(&mut host, &app) {
                                    app.set_error_message(error.to_string().into());
                                    app.set_error_visible(true);
                                }
                                let _ = tx_handle.send(Command::SetVideoWindow(host.handle()));
                                *video_host = Some(host);
                            }
                            Err(error) => {
                                app.set_error_message(error.to_string().into());
                                app.set_error_visible(true);
                            }
                        }
                    }
                    slint::RenderingState::BeforeRendering => {
                        if let Some(host) = video_host.as_mut() {
                            if let Err(error) = sync_video_host(host, &app) {
                                app.set_error_message(error.to_string().into());
                                app.set_error_visible(true);
                            }
                        }
                    }
                    slint::RenderingState::RenderingTeardown => {
                        *video_host = None;
                    }
                    _ => {}
                }
            })
            .ok();

        let timer_app = app.as_weak();
        let timer_video_host = video_host.clone();
        video_resize_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                let Some(app) = timer_app.upgrade() else {
                    return;
                };
                if VideoHost::take_fullscreen_request() {
                    app.window().set_fullscreen(!app.window().is_fullscreen());
                }
                if VideoHost::take_pointer_activity() {
                    app.invoke_reveal_controls();
                }
                let Ok(mut video_host) = timer_video_host.lock() else {
                    return;
                };
                if let Some(host) = video_host.as_mut() {
                    if let Err(error) = sync_video_host(host, &app) {
                        app.set_error_message(error.to_string().into());
                        app.set_error_visible(true);
                    }
                }
            },
        );
    }

    let weak = app.as_weak();

    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            let weak = weak.clone();
            let current_duration = current_duration.clone();

            slint::invoke_from_event_loop(move || {
                let Some(app) = weak.upgrade() else {
                    return;
                };

                match event {
                    Event::LoadStarted(name) => {
                        app.set_file_name(name.into());
                        app.set_audio_media(false);
                        app.set_media_loaded(false);
                        app.set_is_playing(false);
                        app.set_player_phase(PlayerPhase::Loading);
                        app.set_error_visible(false);
                        app.set_error_message("".into());
                    }

                    Event::FileLoaded(name) => {
                        app.set_file_name(name.into());
                        app.set_current_time("00:00".into());
                        app.set_total_time("00:00".into());
                        app.set_duration_seconds(0.0);
                        app.set_progress(0.0);
                        app.set_error_visible(false);
                        if let Ok(mut duration) = current_duration.lock() {
                            *duration = 0.0;
                        }
                    }

                    Event::MediaAvailable(available) => {
                        app.set_media_loaded(available);
                        if !available {
                            app.set_is_playing(false);
                        }
                    }

                    Event::PlaybackState(state) => {
                        app.set_is_playing(state && app.get_media_loaded());
                        if app.get_media_loaded() {
                            app.set_player_phase(if state {
                                PlayerPhase::Playing
                            } else {
                                PlayerPhase::Paused
                            });
                        }
                    }

                    Event::PlaybackStopped => {
                        app.set_player_phase(PlayerPhase::Empty);
                    }

                    Event::PlaybackFinished => {
                        app.set_player_phase(PlayerPhase::Finished);
                    }

                    Event::PlaybackFailed(message) => {
                        app.set_media_loaded(false);
                        app.set_is_playing(false);
                        app.set_player_phase(PlayerPhase::Error);
                        app.set_error_message(message.into());
                        app.set_error_visible(false);
                    }

                    Event::Volume(v) => {
                        app.set_volume((v / 100.0) as f32);
                    }

                    Event::Muted(muted) => {
                        app.set_muted(muted);
                    }

                    Event::Speed(v) => {
                        app.set_speed(v as f32);
                    }

                    Event::TimePos(position) => {
                        app.set_current_time(format_time(position).into());
                        let duration = current_duration
                            .lock()
                            .map(|duration| *duration)
                            .unwrap_or_default();
                        if duration > 0.0 {
                            app.set_progress((position / duration) as f32);
                        }
                    }

                    Event::Duration(duration) => {
                        if let Ok(mut current_duration) = current_duration.lock() {
                            *current_duration = duration;
                        }
                        app.set_total_time(format_time(duration).into());
                        app.set_duration_seconds(duration as f32);
                    }

                    Event::Playlist {
                        items,
                        current_index,
                    } => {
                        let items = items
                            .into_iter()
                            .map(SharedString::from)
                            .collect::<Vec<_>>();
                        let next_file = items.get(current_index + 1).cloned();
                        app.set_playlist_items(ModelRc::new(VecModel::from(items)));
                        app.set_playlist_current_index(
                            i32::try_from(current_index).unwrap_or(i32::MAX),
                        );
                        if let Some(next_file) = next_file {
                            app.set_next_file(next_file);
                            app.set_next_card_visible(true);
                        } else {
                            app.set_next_card_visible(false);
                        }
                    }

                    Event::MediaInfo {
                        width,
                        height,
                        fps,
                        codec,
                        audio_codec,
                    } => {
                        app.set_audio_media(is_audio_only_media(
                            width,
                            height,
                            &codec,
                            &audio_codec,
                        ));
                        app.set_resolution(format!("{}x{}", width, height).into());
                        app.set_fps(format!("{:.2}", fps).into());
                        app.set_codec(codec.into());
                        app.set_audio_codec(audio_codec.into());
                    }

                    Event::LoopEnabled(enabled) => {
                        app.set_loop_enabled(enabled);
                    }

                    Event::FolderLoopEnabled(enabled) => {
                        app.set_folder_loop_enabled(enabled);
                    }

                    Event::ShuffleEnabled(enabled) => {
                        app.set_shuffle_enabled(enabled);
                    }

                    Event::Error(message) => {
                        app.set_error_message(message.into());
                        app.set_error_visible(true);
                    }
                }
            })
            .ok();
        }
    });

    app.run()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        command_for_dropped_path, command_palette_items, format_time, is_audio_only_media,
        shortcut_action, ShortcutAction, PALETTE_COMMANDS,
    };
    use crate::ipc::Command;
    use slint::winit_030::winit::keyboard::{KeyCode, ModifiersState};

    #[test]
    fn formats_short_and_long_durations() {
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(65.0), "01:05");
        assert_eq!(format_time(3_661.0), "01:01:01");
    }

    #[test]
    fn clamps_negative_time_to_zero() {
        assert_eq!(format_time(-10.0), "00:00");
    }

    #[test]
    fn detects_audio_only_media_from_actual_tracks_not_file_extension() {
        assert!(is_audio_only_media(0, 0, "", "MP3 (MPEG audio layer 3)"));
        assert!(!is_audio_only_media(1920, 1080, "yuv420p", "AAC"));
        assert!(!is_audio_only_media(0, 0, "", ""));
    }

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
    }

    #[test]
    fn shortcuts_are_resolved_from_physical_keys_and_exact_modifiers() {
        assert_eq!(
            shortcut_action(KeyCode::KeyK, ModifiersState::empty()),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcut_action(KeyCode::KeyK, ModifiersState::CONTROL),
            Some(ShortcutAction::ToggleCommandPalette)
        );
        assert_eq!(
            shortcut_action(
                KeyCode::KeyO,
                ModifiersState::CONTROL | ModifiersState::SHIFT
            ),
            Some(ShortcutAction::OpenFolder)
        );
        assert_eq!(shortcut_action(KeyCode::KeyK, ModifiersState::ALT), None);
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
