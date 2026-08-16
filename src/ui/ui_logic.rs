use std::sync::{Arc, Mutex};
#[cfg(target_os = "windows")]
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};

use crate::ipc::{Command, Event};
use crate::player::playlist::VIDEO_EXTENSIONS;
#[cfg(target_os = "windows")]
use crate::render::mpv_render::VideoHost;

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

#[cfg(target_os = "windows")]
fn sync_video_host(host: &mut VideoHost, app: &AppWindow) -> Result<()> {
    let video_visible = !app.get_shortcut_overlay_visible() && !app.get_error_visible();
    host.set_visible(video_visible);

    let left_inset = if app.get_info_drawer_visible() {
        340.0
    } else {
        28.0
    };
    let right_inset = if app.get_settings_drawer_visible() {
        320.0
    } else {
        28.0
    };
    host.resize(app.window(), left_inset, right_inset)
}

pub fn run(tx: Sender<Command>, rx: Receiver<Event>) -> Result<()> {
    let app = AppWindow::new()?;
    let current_duration = Arc::new(Mutex::new(0.0));
    #[cfg(target_os = "windows")]
    let video_host = Arc::new(Mutex::new(None::<VideoHost>));
    #[cfg(target_os = "windows")]
    let video_resize_timer = slint::Timer::default();

    {
        let tx = tx.clone();

        app.on_open_file(move || {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Видео", VIDEO_EXTENSIONS)
                .pick_file()
            {
                let _ = tx.send(Command::OpenFile(path.to_string_lossy().to_string()));
            }
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

    {
        let app_weak = app.as_weak();

        app.on_toggle_info(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_info_drawer_visible(!app.get_info_drawer_visible());
            }
        });
    }

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
                    Event::FileLoaded(name) => {
                        app.set_file_name(name.into());
                        app.set_current_time("00:00".into());
                        app.set_total_time("00:00".into());
                        app.set_progress(0.0);
                        app.set_error_visible(false);
                        if let Ok(mut duration) = current_duration.lock() {
                            *duration = 0.0;
                        }
                    }

                    Event::PlaybackState(state) => {
                        app.set_is_playing(state);
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
                    }

                    Event::Playlist {
                        items,
                        current_index,
                    } => {
                        if current_index + 1 < items.len() {
                            app.set_next_file(items[current_index + 1].clone().into());
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
    use super::format_time;

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
}
