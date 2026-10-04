//! Registers UI callbacks that translate player actions into typed commands.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use slint::ComponentHandle;

use crate::{
    ipc::{AspectRatio, Command, EqualizerPreset, FrameStepDirection},
    player::{
        commands::CommandSender,
        playlist::{AUDIO_EXTENSIONS, MEDIA_EXTENSIONS, VIDEO_EXTENSIONS},
    },
};

use super::ui_logic::AppWindow;

const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"];

pub(super) fn register_callbacks(
    app: &AppWindow,
    tx: &CommandSender,
    dialog_active: &Arc<AtomicBool>,
    current_duration: &Arc<Mutex<f64>>,
) {
    register_file_callbacks(app, tx, dialog_active);
    register_playback_callbacks(app, tx, current_duration);
    register_track_callbacks(app, tx, dialog_active);
    register_audio_callbacks(app, tx);
    register_playlist_callbacks(app, tx);
}

fn register_file_callbacks(app: &AppWindow, tx: &CommandSender, dialog_active: &Arc<AtomicBool>) {
    {
        let tx = tx.clone();
        let dialog_active = Arc::clone(dialog_active);
        app.on_open_file(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = Arc::clone(&dialog_active);
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("Медиа", MEDIA_EXTENSIONS)
                    .add_filter("Видео", VIDEO_EXTENSIONS)
                    .add_filter("Аудио", AUDIO_EXTENSIONS)
                    .pick_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::OpenFile(path));
                }
            });
        });
    }

    {
        let tx = tx.clone();
        app.on_open_recent(move |path| {
            let _ = tx.send(Command::OpenFile(PathBuf::from(path.to_string())));
        });
    }

    {
        let tx = tx.clone();
        app.on_clear_playback_data(move || {
            let _ = tx.send(Command::ClearPlaybackData);
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        app.on_resume_saved_playback(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_resume_prompt_visible(false);
            }
            let _ = tx.send(Command::ResolveResume(true));
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        app.on_restart_saved_playback(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_resume_prompt_visible(false);
            }
            let _ = tx.send(Command::ResolveResume(false));
        });
    }

    {
        let tx = tx.clone();
        let dialog_active = Arc::clone(dialog_active);
        app.on_open_folder(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = Arc::clone(&dialog_active);
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new().pick_folder();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::OpenFolder(path));
                }
            });
        });
    }
}

fn register_playback_callbacks(
    app: &AppWindow,
    tx: &CommandSender,
    current_duration: &Arc<Mutex<f64>>,
) {
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
        let current_duration = Arc::clone(current_duration);
        app.on_seek(move |value| {
            let duration = current_duration
                .lock()
                .map(|duration| *duration)
                .unwrap_or_default();
            let _ = tx.send(Command::Seek(f64::from(value) * duration));
        });
    }

    {
        let tx = tx.clone();
        app.on_set_speed(move |value| {
            let _ = tx.send(Command::SetSpeed(f64::from(value)));
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
        let tx = tx.clone();
        app.on_seek_relative(move |value| {
            let _ = tx.send(Command::SeekRelative(f64::from(value)));
        });
    }

    {
        let tx = tx.clone();
        app.on_step_frame(move |direction| {
            let direction = if direction < 0 {
                FrameStepDirection::Backward
            } else {
                FrameStepDirection::Forward
            };
            let _ = tx.send(Command::StepFrame(direction));
        });
    }
}

fn register_track_callbacks(app: &AppWindow, tx: &CommandSender, dialog_active: &Arc<AtomicBool>) {
    {
        let tx = tx.clone();
        let dialog_active = Arc::clone(dialog_active);
        app.on_load_subtitle(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = Arc::clone(&dialog_active);
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("Субтитры", SUBTITLE_EXTENSIONS)
                    .pick_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::LoadSubtitle(path));
                }
            });
        });
    }

    {
        let tx = tx.clone();
        app.on_select_audio_track(move |id| {
            let _ = tx.send(Command::SelectAudioTrack(i64::from(id)));
        });
    }

    {
        let tx = tx.clone();
        app.on_select_subtitle_track(move |id| {
            let id = (id >= 0).then(|| i64::from(id));
            let _ = tx.send(Command::SelectSubtitleTrack(id));
        });
    }

    {
        let tx = tx.clone();
        app.on_select_chapter(move |index| {
            let _ = tx.send(Command::SelectChapter(i64::from(index)));
        });
    }

    {
        let tx = tx.clone();
        app.on_set_aspect_ratio(move |index| {
            if let Some(aspect_ratio) = AspectRatio::from_ui_index(index) {
                let _ = tx.send(Command::SetAspectRatio(aspect_ratio));
            }
        });
    }
}

fn register_audio_callbacks(app: &AppWindow, tx: &CommandSender) {
    {
        let tx = tx.clone();
        app.on_set_volume(move |value| {
            let _ = tx.send(Command::SetVolume(f64::from(value) * 100.0));
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_volume(move |value| {
            let _ = tx.send(Command::AdjustVolume(f64::from(value)));
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
        app.on_adjust_audio_delay(move |delta| {
            let _ = tx.send(Command::AdjustAudioDelay(f64::from(delta)));
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_subtitle_delay(move |delta| {
            let _ = tx.send(Command::AdjustSubtitleDelay(f64::from(delta)));
        });
    }

    {
        let tx = tx.clone();
        app.on_reset_audio_delay(move || {
            let _ = tx.send(Command::ResetAudioDelay);
        });
    }

    {
        let tx = tx.clone();
        app.on_reset_subtitle_delay(move || {
            let _ = tx.send(Command::ResetSubtitleDelay);
        });
    }

    {
        let tx = tx.clone();
        app.on_set_equalizer_enabled(move |enabled| {
            let _ = tx.send(Command::SetEqualizerEnabled(enabled));
        });
    }

    {
        let tx = tx.clone();
        app.on_set_equalizer_preset(move |index| {
            if let Some(preset) = EqualizerPreset::from_ui_index(index) {
                let _ = tx.send(Command::SetEqualizerPreset(preset));
            }
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_equalizer_preamp(move |delta| {
            let _ = tx.send(Command::AdjustEqualizerPreamp(f64::from(delta)));
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_equalizer_band(move |index, delta| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let _ = tx.send(Command::AdjustEqualizerBand {
                index,
                delta_db: f64::from(delta),
            });
        });
    }

    {
        let tx = tx.clone();
        app.on_reset_equalizer(move || {
            let _ = tx.send(Command::ResetEqualizer);
        });
    }
}

fn register_playlist_callbacks(app: &AppWindow, tx: &CommandSender) {
    {
        let tx = tx.clone();
        app.on_play_playlist_index(move |index| {
            if let Ok(index) = usize::try_from(index) {
                let _ = tx.send(Command::PlayIndex(index));
            }
        });
    }

    {
        let tx = tx.clone();
        app.on_remove_playlist_index(move |index| {
            if let Ok(index) = usize::try_from(index) {
                let _ = tx.send(Command::RemoveIndex(index));
            }
        });
    }
}
