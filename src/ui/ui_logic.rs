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
use crate::player::playlist::{is_video_file, VIDEO_EXTENSIONS};
#[cfg(target_os = "windows")]
use crate::render::mpv_render::VideoHost;
use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
use slint::{ModelRc, SharedString, VecModel};

slint::include_modules!();

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

fn command_for_dropped_path(path: &Path) -> Result<Command, String> {
    if path.is_dir() {
        return Ok(Command::OpenFolder(path.to_string_lossy().into_owned()));
    }
    if path.is_file() && is_video_file(path) {
        return Ok(Command::OpenFile(path.to_string_lossy().into_owned()));
    }

    Err(format!(
        "Перетащенный объект не является поддерживаемым видео или папкой: {}",
        path.display()
    ))
}

#[cfg(target_os = "windows")]
fn sync_video_host(host: &mut VideoHost, app: &AppWindow) -> Result<()> {
    let video_visible = app.get_media_loaded()
        && !app.get_shortcut_overlay_visible()
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
    host.resize(app.window(), left_inset, right_inset, 76.0, 116.0)
}

pub fn run(tx: Sender<Command>, rx: Receiver<Event>) -> Result<()> {
    let app = AppWindow::new()?;
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

        app.window().on_winit_window_event(move |_, event| {
            let Some(app) = app_weak.upgrade() else {
                return EventResult::Propagate;
            };

            match event {
                WindowEvent::HoveredFile(path) => {
                    app.set_drop_target_visible(path.is_dir() || is_video_file(path));
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
                    .add_filter("Видео", VIDEO_EXTENSIONS)
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
                app.set_shortcut_overlay_visible(!app.get_shortcut_overlay_visible());
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
                        app.set_buffered_progress(0.0);
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

                    Event::BufferedUntil(position) => {
                        let duration = current_duration
                            .lock()
                            .map(|duration| *duration)
                            .unwrap_or_default();
                        if duration > 0.0 {
                            app.set_buffered_progress((position / duration).clamp(0.0, 1.0) as f32);
                        }
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

    use super::{command_for_dropped_path, format_time};
    use crate::ipc::Command;

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
    fn rejects_unsupported_dropped_paths() {
        assert!(command_for_dropped_path(Path::new("notes.txt")).is_err());
        assert!(command_for_dropped_path(Path::new("missing.mp4")).is_err());
    }

    #[test]
    fn creates_open_file_command_for_supported_drop() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("oxide-drop-{unique}.mkv"));
        std::fs::write(&path, []).expect("temporary video must be created");

        assert!(matches!(
            command_for_dropped_path(&path),
            Ok(Command::OpenFile(_))
        ));

        std::fs::remove_file(path).expect("temporary video must be removed");
    }
}
