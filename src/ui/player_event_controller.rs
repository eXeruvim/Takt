//! Applies bounded player-event batches to the Slint presentation model.

use std::sync::{Arc, Mutex};

use crossbeam_channel::Receiver;
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::ipc::{AspectRatio, Event};

#[cfg(target_os = "windows")]
use crate::player::commands::CommandSender;

use super::{
    event_delivery::{self, Bridge},
    presentation::{
        format_bitrate, format_codec, format_container, format_file_size, format_precise_file_size,
        format_time, format_track_summary, is_audio_only_media, media_name_without_extension,
    },
    ui_logic::{
        AppWindow, AudioTrackEntry, ChapterEntry, EqualizerBandEntry, PlayerPhase, RecentFileEntry,
        SubtitleTrackEntry,
    },
};

#[cfg(target_os = "windows")]
use super::video_output::VideoOutputEventHandle;

#[derive(Clone)]
pub(super) struct PlayerEventController {
    current_duration: Arc<Mutex<f64>>,
    #[cfg(target_os = "windows")]
    video_output: VideoOutputEventHandle,
    #[cfg(target_os = "windows")]
    player_tx: CommandSender,
}

impl PlayerEventController {
    pub(super) fn new(
        #[cfg(target_os = "windows")] video_output: VideoOutputEventHandle,
        #[cfg(target_os = "windows")] player_tx: CommandSender,
    ) -> Self {
        Self {
            current_duration: Arc::new(Mutex::new(0.0)),
            #[cfg(target_os = "windows")]
            video_output,
            #[cfg(target_os = "windows")]
            player_tx,
        }
    }

    pub(super) fn duration_handle(&self) -> Arc<Mutex<f64>> {
        Arc::clone(&self.current_duration)
    }

    pub(super) fn spawn_bridge(
        &self,
        app: &AppWindow,
        rx: Receiver<Event>,
    ) -> std::io::Result<Bridge> {
        let weak = app.as_weak();
        let controller = self.clone();
        event_delivery::spawn(
            rx,
            |previous, next| matches!((previous, next), (Event::TimePos(_), Event::TimePos(_))),
            move |batch, completion| {
                let weak = weak.clone();
                let controller = controller.clone();
                slint::invoke_from_event_loop(move || {
                    let _completion = completion;
                    let Some(app) = weak.upgrade() else {
                        return;
                    };
                    for event in batch {
                        controller.apply(&app, event);
                    }
                })
                .is_ok()
            },
        )
    }

    fn apply(&self, app: &AppWindow, event: Event) {
        match event {
            #[cfg(target_os = "windows")]
            Event::RenderSource(source) => self.video_output.set_render_source(app, source),

            #[cfg(not(target_os = "windows"))]
            Event::RenderSource(_) => {}

            Event::VideoRendererRequired => {
                #[cfg(target_os = "windows")]
                self.video_output.request_renderer(app, &self.player_tx);
            }

            Event::LoadStarted(name) => {
                app.set_resume_prompt_visible(false);
                let preserve_media_surface = app.get_media_presented();
                app.set_audio_display_name(media_name_without_extension(&name).into());
                app.set_file_name(name.into());
                app.set_track_title("".into());
                app.set_track_artist("".into());
                app.set_album_title("".into());
                app.set_album_art(Image::default());
                app.set_album_art_available(false);
                app.set_audio_tracks(ModelRc::new(VecModel::from(Vec::<AudioTrackEntry>::new())));
                app.set_subtitle_tracks(ModelRc::new(VecModel::from(
                    Vec::<SubtitleTrackEntry>::new(),
                )));
                app.set_subtitles_enabled(false);
                app.set_audio_delay(0.0);
                app.set_subtitle_delay(0.0);
                app.set_chapters(ModelRc::new(VecModel::from(Vec::<ChapterEntry>::new())));
                app.set_current_chapter_index(-1);
                app.set_aspect_ratio_index(AspectRatio::Original.ui_index());
                app.set_waiting_for_video_surface(false);
                app.set_media_transitioning(preserve_media_surface);
                app.set_is_playing(false);
                if !preserve_media_surface {
                    app.set_audio_media(false);
                    app.set_media_loaded(false);
                    app.set_player_phase(PlayerPhase::Loading);
                }
                app.set_error_visible(false);
                app.set_error_message("".into());
            }

            Event::FileLoaded { name, path } => {
                app.set_audio_display_name(media_name_without_extension(&name).into());
                app.set_file_name(name.clone().into());
                let path = path.to_string_lossy().into_owned();
                app.set_current_media_path(path.clone().into());
                if !app.get_conversion_running() && !app.get_converter_visible() {
                    app.set_conversion_source_path(path.into());
                    app.set_conversion_source_name(name.into());
                    app.set_conversion_source_duration("—".into());
                    app.set_conversion_source_duration_seconds(0.0);
                    app.set_conversion_source_format("—".into());
                    app.set_conversion_source_size("—".into());
                }
                if !app.get_conversion_running() {
                    app.set_conversion_output_path("".into());
                    app.set_conversion_progress(0.0);
                    app.set_conversion_status("".into());
                    app.set_conversion_size_status("".into());
                }
                app.set_current_time("00:00".into());
                app.set_total_time("00:00".into());
                app.set_duration_seconds(0.0);
                app.set_progress(0.0);
                app.set_container_format("—".into());
                app.set_file_size_label("—".into());
                app.set_media_duration_label("—".into());
                app.set_media_bitrate_label("—".into());
                app.set_track_summary("—".into());
                app.set_error_visible(false);
                if let Ok(mut duration) = self.current_duration.lock() {
                    *duration = 0.0;
                }
            }

            Event::MediaAvailable(available) => {
                app.set_media_loaded(available);
                app.set_media_transitioning(available && app.get_waiting_for_video_surface());
                if !available {
                    app.set_waiting_for_video_surface(false);
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
                app.set_resume_prompt_visible(false);
                app.set_media_transitioning(false);
                app.set_player_phase(PlayerPhase::Empty);
            }

            Event::PlaybackFinished => {
                app.set_resume_prompt_visible(false);
                app.set_media_transitioning(false);
                app.set_player_phase(PlayerPhase::Finished);
            }

            Event::PlaybackFailed(message) => {
                app.set_resume_prompt_visible(false);
                app.set_media_transitioning(false);
                app.set_waiting_for_video_surface(false);
                app.set_media_loaded(false);
                app.set_is_playing(false);
                app.set_player_phase(PlayerPhase::Error);
                app.set_error_message(message.into());
                app.set_error_visible(false);
            }

            Event::Volume(value) => {
                app.set_volume((value / 100.0) as f32);
                app.invoke_schedule_playback_defaults_save();
            }
            Event::Muted(muted) => app.set_muted(muted),
            Event::Speed(value) => {
                app.set_speed(value as f32);
                app.invoke_schedule_playback_defaults_save();
            }
            Event::AudioDelay(value) => app.set_audio_delay(value as f32),
            Event::SubtitleDelay(value) => app.set_subtitle_delay(value as f32),

            Event::TimePos(position) => {
                // Keep session state advancing while minimized without waking
                // Slint and the OpenGL renderer for invisible timeline frames.
                if !app.get_window_minimized() {
                    app.set_current_time(format_time(position).into());
                    let duration = self
                        .current_duration
                        .lock()
                        .map(|duration| *duration)
                        .unwrap_or_default();
                    if duration > 0.0 {
                        app.set_progress((position / duration) as f32);
                    }
                }
            }

            Event::Duration(duration) => {
                if let Ok(mut current_duration) = self.current_duration.lock() {
                    *current_duration = duration;
                }
                app.set_total_time(format_time(duration).into());
                app.set_duration_seconds(duration as f32);
                app.set_media_duration_label(if duration.is_finite() && duration > 0.0 {
                    format_time(duration).into()
                } else {
                    "—".into()
                });
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
                app.set_playlist_current_index(i32::try_from(current_index).unwrap_or(i32::MAX));
                app.set_playlist_focus_revision(app.get_playlist_focus_revision().wrapping_add(1));
                if let Some(next_file) = next_file {
                    app.set_next_file(next_file);
                    app.set_next_card_visible(true);
                } else {
                    app.set_next_card_visible(false);
                }
            }

            Event::RecentFiles(files) => {
                if !files.is_empty() {
                    app.set_playback_data_cleared(false);
                }
                let files = files
                    .into_iter()
                    .map(|file| RecentFileEntry {
                        name: file.name.into(),
                        path: file.path.to_string_lossy().into_owned().into(),
                        location: file.location.into(),
                    })
                    .collect::<Vec<_>>();
                app.set_recent_files(ModelRc::new(VecModel::from(files)));
            }

            Event::ResumeAvailable(position) => {
                app.set_resume_position_label(format_time(position).into());
                app.set_resume_prompt_visible(true);
            }

            Event::PlaybackDataCleared => {
                app.set_resume_prompt_visible(false);
                app.set_clear_playback_data_prompt_visible(false);
                app.set_playback_data_cleared(true);
                app.set_notice_message("История очищена".into());
                app.set_notice_visible(true);
                let notice_app = app.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_millis(2600), move || {
                    if let Some(app) = notice_app.upgrade() {
                        app.set_notice_visible(false);
                    }
                });
            }

            Event::MediaInfo {
                width,
                height,
                fps,
                codec,
                audio_codec,
                container,
                file_size_bytes,
                duration_seconds,
                average_bitrate_bps,
                video_track_count,
                audio_track_count,
                subtitle_track_count,
            } => self.apply_media_info(
                app,
                MediaInfoEvent {
                    width,
                    height,
                    fps,
                    codec,
                    audio_codec,
                    container,
                    file_size_bytes,
                    duration_seconds,
                    average_bitrate_bps,
                    video_track_count,
                    audio_track_count,
                    subtitle_track_count,
                },
            ),

            Event::AudioMetadata {
                title,
                artist,
                album,
                artwork_rgba,
                artwork_width,
                artwork_height,
            } => {
                app.set_track_title(title.into());
                app.set_track_artist(artist.into());
                app.set_album_title(album.into());

                let expected_len = usize::try_from(artwork_width)
                    .ok()
                    .and_then(|width| {
                        usize::try_from(artwork_height)
                            .ok()
                            .and_then(|height| width.checked_mul(height))
                    })
                    .and_then(|pixels| pixels.checked_mul(4));
                if expected_len == Some(artwork_rgba.len()) && !artwork_rgba.is_empty() {
                    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        &artwork_rgba,
                        artwork_width,
                        artwork_height,
                    );
                    app.set_album_art(Image::from_rgba8(buffer));
                    app.set_album_art_available(true);
                } else {
                    app.set_album_art(Image::default());
                    app.set_album_art_available(false);
                }
            }

            Event::AudioTracks(tracks) => {
                let tracks = tracks
                    .into_iter()
                    .filter_map(|track| {
                        Some(AudioTrackEntry {
                            id: i32::try_from(track.id).ok()?,
                            label: track.label.into(),
                            selected: track.selected,
                        })
                    })
                    .collect::<Vec<_>>();
                app.set_audio_tracks(ModelRc::new(VecModel::from(tracks)));
            }

            Event::SubtitleTracks(tracks) => {
                let tracks = tracks
                    .into_iter()
                    .filter_map(|track| {
                        Some(SubtitleTrackEntry {
                            id: i32::try_from(track.id).ok()?,
                            label: track.label.into(),
                            selected: track.selected,
                        })
                    })
                    .collect::<Vec<_>>();
                app.set_subtitles_enabled(tracks.iter().any(|track| track.selected));
                app.set_subtitle_tracks(ModelRc::new(VecModel::from(tracks)));
            }

            Event::Chapters {
                items,
                current_index,
            } => {
                let chapters = items
                    .into_iter()
                    .filter_map(|chapter| {
                        Some(ChapterEntry {
                            index: i32::try_from(chapter.index).ok()?,
                            label: chapter.label.into(),
                        })
                    })
                    .collect::<Vec<_>>();
                app.set_chapters(ModelRc::new(VecModel::from(chapters)));
                app.set_current_chapter_index(i32::try_from(current_index).unwrap_or(-1));
            }

            Event::ChapterChanged(index) => {
                app.set_current_chapter_index(i32::try_from(index).unwrap_or(-1));
            }
            Event::AspectRatioChanged(aspect_ratio) => {
                app.set_aspect_ratio_index(aspect_ratio.ui_index());
            }
            Event::HardwareDecoding { enabled, decoder } => {
                app.set_hardware_decoding_enabled(enabled);
                app.set_hardware_decoder(decoder.into());
            }
            Event::EqualizerChanged {
                enabled,
                preset,
                preamp_db,
                band_gains_db,
            } => {
                app.set_equalizer_enabled(enabled);
                app.set_equalizer_preset_index(preset.ui_index());
                app.set_equalizer_preamp_db(preamp_db as f32);
                let bands = ["60 Гц", "230 Гц", "910 Гц", "3,6 кГц", "14 кГц"]
                    .into_iter()
                    .zip(band_gains_db)
                    .map(|(label, gain_db)| EqualizerBandEntry {
                        label: label.into(),
                        gain_db: gain_db as f32,
                    })
                    .collect::<Vec<_>>();
                app.set_equalizer_bands(ModelRc::new(VecModel::from(bands)));
            }
            Event::VideoOutputReady => {
                if app.get_waiting_for_video_surface() {
                    app.set_audio_media(false);
                    app.set_waiting_for_video_surface(false);
                    app.set_media_transitioning(false);
                }
            }
            Event::LoopEnabled(enabled) => app.set_loop_enabled(enabled),
            Event::FolderLoopEnabled(enabled) => app.set_folder_loop_enabled(enabled),
            Event::ShuffleEnabled(enabled) => app.set_shuffle_enabled(enabled),
            Event::Error(message) => {
                app.set_error_message(message.into());
                app.set_error_visible(true);
            }
        }
    }

    fn apply_media_info(&self, app: &AppWindow, info: MediaInfoEvent) {
        let audio_media =
            is_audio_only_media(info.video_track_count, &info.codec, &info.audio_codec);
        let wait_for_video = app.get_media_transitioning() && app.get_audio_media() && !audio_media;
        app.set_waiting_for_video_surface(wait_for_video);
        if !wait_for_video {
            app.set_audio_media(audio_media);
        }
        app.set_resolution(if info.width > 0 && info.height > 0 {
            format!("{}×{}", info.width, info.height).into()
        } else {
            "—".into()
        });
        app.set_fps(if info.fps.is_finite() && info.fps > 0.0 {
            format!("{:.2} FPS", info.fps).into()
        } else {
            "—".into()
        });
        app.set_codec(format_codec(&info.codec).into());
        app.set_audio_codec(format_codec(&info.audio_codec).into());
        if !app.get_conversion_running() && !app.get_converter_visible() {
            super::conversion_controller::apply_presets(app, audio_media, &info.audio_codec);
            app.set_conversion_audio_media(audio_media);
            app.set_conversion_source_duration_seconds(info.duration_seconds.max(0.0) as f32);
            app.set_conversion_source_duration(
                if info.duration_seconds.is_finite() && info.duration_seconds > 0.0 {
                    format_time(info.duration_seconds).into()
                } else {
                    "—".into()
                },
            );
            app.set_conversion_source_format(if audio_media {
                format_codec(&info.audio_codec).into()
            } else {
                format_container(&info.container).into()
            });
            app.set_conversion_source_size(format_precise_file_size(info.file_size_bytes).into());
        }
        app.set_container_format(format_container(&info.container).into());
        app.set_file_size_label(format_file_size(info.file_size_bytes).into());
        app.set_media_duration_label(
            if info.duration_seconds.is_finite() && info.duration_seconds > 0.0 {
                format_time(info.duration_seconds).into()
            } else {
                "—".into()
            },
        );
        app.set_media_bitrate_label(format_bitrate(info.average_bitrate_bps).into());
        app.set_track_summary(
            format_track_summary(
                info.video_track_count,
                info.audio_track_count,
                info.subtitle_track_count,
            )
            .into(),
        );
    }
}

struct MediaInfoEvent {
    width: u32,
    height: u32,
    fps: f64,
    codec: String,
    audio_codec: String,
    container: String,
    file_size_bytes: u64,
    duration_seconds: f64,
    average_bitrate_bps: f64,
    video_track_count: u32,
    audio_track_count: u32,
    subtitle_track_count: u32,
}
