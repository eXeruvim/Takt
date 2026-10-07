use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::ipc::{Command, Event, FrameStepDirection, RecentFileInfo};
use crate::player::{
    background_jobs::{BackgroundJobs, ReadyPlaylist},
    commands::{CommandReceiver, DISPATCH_BUDGET},
    equalizer::EqualizerSettings,
    limits::{PATH_TEXT_BYTES, UI_TEXT_BYTES, bounded_text},
    media_info::{MediaInfo, read_audio_tracks, read_chapters, read_subtitle_tracks},
    metadata_worker::MetadataResult,
    mpv_adapter::{
        automatic_hwdec_mode, create as create_mpv, hwdec_is_enabled, mpv_error,
        observe_playback_properties, set_loaded_media_paused, video_background_color,
    },
    playback_diagnostics::PlaybackDiagnostics,
    playback_state::PlaybackState,
    playlist::DirectoryPlaylist,
    session_store::SessionStore,
};
use crate::render::mpv_opengl::MpvRenderSource;
use anyhow::Context;
use crossbeam_channel::Sender;
use libmpv::{
    Mpv,
    events::{Event as MpvEvent, PropertyData},
    mpv_end_file_reason,
};

const PREVIOUS_RESTART_THRESHOLD_SECONDS: f64 = 3.0;
const MAX_SYNC_DELAY_SECONDS: f64 = 30.0;
const SESSION_SAVE_INTERVAL: Duration = Duration::from_secs(10);
#[derive(Default)]
struct PlaybackOptions {
    repeat_folder: bool,
    shuffled: bool,
    media_available: bool,
    equalizer: EqualizerSettings,
}

fn loop_value_is_enabled(value: &str) -> bool {
    !matches!(value, "" | "no" | "0" | "false")
}

fn should_restart_current(position: f64) -> bool {
    position.is_finite() && position > PREVIOUS_RESTART_THRESHOLD_SECONDS
}

fn adjusted_delay(current: f64, delta: f64) -> f64 {
    ((current + delta).clamp(-MAX_SYNC_DELAY_SECONDS, MAX_SYNC_DELAY_SECONDS) * 10.0).round() / 10.0
}

fn send_hardware_decoding(mpv: &Mpv, tx: &Sender<Event>) {
    let requested = mpv
        .get_property::<String>("hwdec")
        .unwrap_or_else(|_| "no".to_owned());
    let decoder = mpv
        .get_property::<String>("hwdec-current")
        .unwrap_or_else(|_| "no".to_owned());
    let _ = tx.send(Event::HardwareDecoding {
        enabled: hwdec_is_enabled(&requested),
        decoder: bounded_text(&decoder, UI_TEXT_BYTES),
    });
}

fn send_equalizer(settings: EqualizerSettings, tx: &Sender<Event>) {
    let _ = tx.send(Event::EqualizerChanged {
        enabled: settings.enabled,
        preset: settings.preset,
        preamp_db: settings.preamp_db,
        band_gains_db: settings.band_gains_db(),
    });
}

fn remove_equalizer_filter(mpv: &Mpv) -> anyhow::Result<()> {
    mpv_error(mpv.command("change-list", &["af", "remove", "@oxide-eq"]))
        .context("Не удалось удалить аудиофильтр Oxide")
}

fn append_equalizer_filter(mpv: &Mpv, settings: EqualizerSettings) -> anyhow::Result<()> {
    let Some(filter) = settings.mpv_filter() else {
        return Ok(());
    };

    // `append` accepts one unescaped object-list item, so commas inside the
    // lavfi graph stay part of this named filter instead of splitting `af`.
    mpv_error(mpv.command("change-list", &["af", "append", &filter]))
        .context("Не удалось применить аудиофильтр Oxide")
}

fn replace_equalizer_filter(
    mpv: &Mpv,
    previous: EqualizerSettings,
    next: EqualizerSettings,
) -> anyhow::Result<()> {
    remove_equalizer_filter(mpv)?;
    if let Err(error) = append_equalizer_filter(mpv, next) {
        // Keep the audible state and the UI state in sync if a preset cannot
        // be constructed by the libavfilter build bundled with mpv.
        let _ = remove_equalizer_filter(mpv);
        let _ = append_equalizer_filter(mpv, previous);
        return Err(error);
    }
    Ok(())
}

fn update_equalizer(
    mpv: &Mpv,
    tx: &Sender<Event>,
    options: &mut PlaybackOptions,
    next: EqualizerSettings,
) -> anyhow::Result<()> {
    replace_equalizer_filter(mpv, options.equalizer, next)?;
    options.equalizer = next;
    send_equalizer(next, tx);
    Ok(())
}

fn send_playlist(tx: &Sender<Event>, playlist: &DirectoryPlaylist) {
    let _ = tx.send(Event::Playlist {
        items: playlist.items(),
        current_index: playlist.current_index(),
    });
}

fn send_recent_files(tx: &Sender<Event>, session: &SessionStore) {
    let files = session
        .recent()
        .iter()
        .filter(|path| path.is_file())
        .map(|path| RecentFileInfo {
            path: path.clone(),
            name: path
                .file_name()
                .map(|name| bounded_text(&name.to_string_lossy(), UI_TEXT_BYTES))
                .unwrap_or_else(|| bounded_text(&path.to_string_lossy(), UI_TEXT_BYTES)),
            location: path
                .parent()
                .map(|parent| bounded_text(&parent.to_string_lossy(), PATH_TEXT_BYTES))
                .unwrap_or_default(),
        })
        .collect();
    let _ = tx.send(Event::RecentFiles(files));
}

fn record_current_media(
    tx: &Sender<Event>,
    playlist: &Option<DirectoryPlaylist>,
    session: &mut SessionStore,
) {
    if let Some(path) = playlist.as_ref().and_then(DirectoryPlaylist::current_path) {
        session.record_open(path);
        save_session(tx, session);
        send_recent_files(tx, session);
    }
}

fn save_session(tx: &Sender<Event>, session: &mut SessionStore) {
    if let Some(message) = session.save_in_background_and_take_error() {
        eprintln!("{message}");
        let _ = tx.send(Event::Error(message));
    }
}

fn flush_session(tx: &Sender<Event>, session: &mut SessionStore) {
    if let Some(message) = session.save_and_take_error() {
        eprintln!("{message}");
        let _ = tx.send(Event::Error(message));
    }
}

fn send_media_info(info: &MediaInfo, tx: &Sender<Event>) {
    let _ = tx.send(Event::MediaInfo {
        width: info.width,
        height: info.height,
        fps: info.fps,
        codec: info.codec.clone(),
        audio_codec: info.audio_codec.clone(),
        container: info.container.clone(),
        file_size_bytes: info.file_size_bytes,
        duration_seconds: info.duration_seconds,
        average_bitrate_bps: info.average_bitrate_bps,
        video_track_count: info.video_track_count,
        audio_track_count: info.audio_track_count,
        subtitle_track_count: info.subtitle_track_count,
    });
}

fn audio_metadata_is_relevant(info: &MediaInfo) -> bool {
    info.video_track_count == 0 && info.audio_track_count > 0
}

fn send_media_ready(mpv: &Mpv, tx: &Sender<Event>) -> MediaInfo {
    let info = MediaInfo::read(mpv);
    send_media_info(&info, tx);
    if info.video_track_count > 0 {
        let _ = tx.send(Event::VideoRendererRequired);
    }
    let _ = tx.send(Event::AudioTracks(read_audio_tracks(mpv)));
    let _ = tx.send(Event::SubtitleTracks(read_subtitle_tracks(mpv)));
    let _ = tx.send(Event::Chapters {
        items: read_chapters(mpv),
        current_index: mpv.get_property::<i64>("chapter").unwrap_or(-1),
    });
    send_hardware_decoding(mpv, tx);
    let _ = tx.send(Event::MediaAvailable(true));
    let paused = mpv.get_property::<bool>("pause").unwrap_or(false);
    let _ = tx.send(Event::PlaybackState(!paused));
    info
}

fn send_audio_metadata(result: MetadataResult, tx: &Sender<Event>) {
    let metadata = result.metadata;
    let (artwork_rgba, artwork_width, artwork_height) = metadata
        .artwork
        .map(|artwork| (artwork.rgba, artwork.width, artwork.height))
        .unwrap_or_default();

    let _ = tx.send(Event::AudioMetadata {
        title: metadata.title,
        artist: metadata.artist,
        album: metadata.album,
        artwork_rgba,
        artwork_width,
        artwork_height,
    });
}

fn quote_mpv_command_arg(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn load_file_in_mpv(mpv: &Mpv, path: &Path) -> anyhow::Result<()> {
    let quoted_path = quote_mpv_command_arg(&path.to_string_lossy());
    mpv_error(mpv.command("loadfile", &[&quoted_path, "replace"]))
        .with_context(|| format!("Не удалось открыть медиафайл: {}", path.display()))
}

fn activate_playlist(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    mut next_playlist: DirectoryPlaylist,
    shuffled: bool,
) -> anyhow::Result<()> {
    let limit_notice = next_playlist.limit_notice();
    next_playlist.set_shuffled(shuffled);
    load_current(mpv, tx, &next_playlist)?;
    *playlist = Some(next_playlist);
    if let Some(message) = limit_notice {
        let _ = tx.send(Event::Error(message));
    }
    Ok(())
}

fn replace_playlist_without_reload(
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    mut next_playlist: DirectoryPlaylist,
    shuffled: bool,
) {
    let limit_notice = next_playlist.limit_notice();
    next_playlist.set_shuffled(shuffled);
    send_playlist(tx, &next_playlist);
    *playlist = Some(next_playlist);
    if let Some(message) = limit_notice {
        let _ = tx.send(Event::Error(message));
    }
}

fn load_current(mpv: &Mpv, tx: &Sender<Event>, playlist: &DirectoryPlaylist) -> anyhow::Result<()> {
    let path = playlist
        .current_path()
        .context("В плейлисте нет медиафайла для воспроизведения")?;
    if let Some(file_name) = path.file_name() {
        let _ = tx.send(Event::LoadStarted(bounded_text(
            &file_name.to_string_lossy(),
            UI_TEXT_BYTES,
        )));
    }
    load_file_in_mpv(mpv, path)?;

    if let Some(file_name) = path.file_name() {
        let _ = tx.send(Event::FileLoaded {
            name: bounded_text(&file_name.to_string_lossy(), UI_TEXT_BYTES),
            path: path.to_owned(),
        });
    }
    send_playlist(tx, playlist);
    Ok(())
}

fn load_adjacent(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    next: bool,
    wrap: bool,
) -> anyhow::Result<bool> {
    let selected = playlist.as_mut().and_then(|playlist| {
        if next {
            playlist.move_next(wrap)
        } else {
            playlist.move_previous(wrap)
        }
    });

    if selected.is_none() {
        return Ok(false);
    }

    load_selected_or_skip_missing(mpv, tx, playlist, next, wrap)
}

fn load_selected_or_skip_missing(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    next: bool,
    wrap: bool,
) -> anyhow::Result<bool> {
    loop {
        let current_exists = playlist
            .as_ref()
            .and_then(DirectoryPlaylist::current_path)
            .is_some_and(|path| path.is_file());

        if current_exists {
            let playlist = playlist
                .as_ref()
                .context("Плейлист исчез во время перехода")?;
            load_current(mpv, tx, playlist)?;
            return Ok(true);
        }

        let selected = playlist
            .as_mut()
            .and_then(|playlist| playlist.remove_current_and_select(next, wrap));
        if selected.is_none() {
            if let Some(playlist) = playlist.as_ref() {
                send_playlist(tx, playlist);
            }
            return Ok(false);
        }
    }
}

fn skip_failed_current(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    wrap: bool,
) -> anyhow::Result<bool> {
    let selected = playlist
        .as_mut()
        .and_then(|playlist| playlist.remove_current_and_select(true, wrap));
    if selected.is_none() {
        if let Some(playlist) = playlist.as_ref() {
            send_playlist(tx, playlist);
        }
        return Ok(false);
    }

    load_selected_or_skip_missing(mpv, tx, playlist, true, wrap)
}

fn send_playback_finished(tx: &Sender<Event>) {
    let _ = tx.send(Event::MediaAvailable(false));
    let _ = tx.send(Event::PlaybackState(false));
    let _ = tx.send(Event::PlaybackFinished);
}

fn send_playback_failed(tx: &Sender<Event>, message: String) {
    let _ = tx.send(Event::MediaAvailable(false));
    let _ = tx.send(Event::PlaybackState(false));
    let _ = tx.send(Event::PlaybackFailed(bounded_text(&message, UI_TEXT_BYTES)));
}

fn handle_command(
    command: Command,
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    options: &mut PlaybackOptions,
) -> anyhow::Result<bool> {
    match command {
        Command::OpenFile(_) | Command::OpenFolder(_) => {
            anyhow::bail!("Команда открытия не была передана worker формирования очереди");
        }
        Command::TogglePlay => {
            if !options.media_available {
                return Ok(true);
            }
            let paused = mpv.get_property::<bool>("pause").unwrap_or(false);
            mpv_error(mpv.set_property("pause", !paused))?;
        }
        Command::Stop => {
            if !options.media_available {
                return Ok(true);
            }
            mpv_error(mpv.command("stop", &[]))?;
            options.media_available = false;
            let _ = tx.send(Event::MediaAvailable(false));
            let _ = tx.send(Event::PlaybackState(false));
            let _ = tx.send(Event::TimePos(0.0));
            let _ = tx.send(Event::PlaybackStopped);
        }
        Command::Seek(position) => {
            if options.media_available {
                mpv_error(mpv.set_property("time-pos", position.max(0.0)))?;
            }
        }
        Command::SetVolume(value) => {
            mpv_error(mpv.set_property("volume", value.clamp(0.0, 100.0)))?;
        }
        Command::SetSpeed(value) => {
            mpv_error(mpv.set_property("speed", value.clamp(0.25, 4.0)))?;
        }
        Command::SelectAudioTrack(id) => {
            if !read_audio_tracks(mpv).iter().any(|track| track.id == id) {
                anyhow::bail!("Аудиодорожка {id} отсутствует в текущем файле");
            }
            mpv_error(mpv.set_property("aid", id))?;
            let _ = tx.send(Event::AudioTracks(read_audio_tracks(mpv)));
        }
        Command::SelectSubtitleTrack(id) => {
            if let Some(id) = id {
                if !read_subtitle_tracks(mpv).iter().any(|track| track.id == id) {
                    anyhow::bail!("Субтитры {id} отсутствуют в текущем файле");
                }
                mpv_error(mpv.set_property("sid", id))?;
            } else {
                mpv_error(mpv.set_property("sid", "no".to_owned()))?;
            }
            let _ = tx.send(Event::SubtitleTracks(read_subtitle_tracks(mpv)));
        }
        Command::LoadSubtitle(path) => {
            if !options.media_available {
                return Ok(true);
            }
            if !path.is_file() {
                anyhow::bail!("Файл субтитров не найден: {}", path.display());
            }
            let quoted_path = quote_mpv_command_arg(&path.to_string_lossy());
            mpv_error(mpv.command("sub-add", &[&quoted_path, "select"]))
                .with_context(|| format!("Не удалось загрузить субтитры: {}", path.display()))?;
            let _ = tx.send(Event::SubtitleTracks(read_subtitle_tracks(mpv)));
        }
        Command::AdjustAudioDelay(delta) => {
            if options.media_available {
                let current = mpv.get_property::<f64>("audio-delay").unwrap_or_default();
                mpv_error(mpv.set_property("audio-delay", adjusted_delay(current, delta)))?;
            }
        }
        Command::AdjustSubtitleDelay(delta) => {
            if options.media_available {
                let current = mpv.get_property::<f64>("sub-delay").unwrap_or_default();
                mpv_error(mpv.set_property("sub-delay", adjusted_delay(current, delta)))?;
            }
        }
        Command::ResetAudioDelay => {
            if options.media_available {
                mpv_error(mpv.set_property("audio-delay", 0.0))?;
            }
        }
        Command::ResetSubtitleDelay => {
            if options.media_available {
                mpv_error(mpv.set_property("sub-delay", 0.0))?;
            }
        }
        Command::SelectChapter(index) => {
            if !read_chapters(mpv)
                .iter()
                .any(|chapter| chapter.index == index)
            {
                anyhow::bail!("Глава {} отсутствует в текущем файле", index + 1);
            }
            mpv_error(mpv.set_property("chapter", index))?;
            let _ = tx.send(Event::ChapterChanged(index));
        }
        Command::SetAspectRatio(aspect_ratio) => {
            if options.media_available {
                mpv_error(
                    mpv.set_property("video-aspect-override", aspect_ratio.mpv_value().to_owned()),
                )?;
                let _ = tx.send(Event::AspectRatioChanged(aspect_ratio));
            }
        }
        Command::SetHardwareDecoding(enabled) => {
            mpv_error(
                mpv.set_property(
                    "hwdec",
                    if enabled {
                        automatic_hwdec_mode()
                    } else {
                        "no"
                    }
                    .to_owned(),
                ),
            )?;
            send_hardware_decoding(mpv, tx);
        }
        Command::StepFrame(direction) => {
            if options.media_available && MediaInfo::read(mpv).width > 0 {
                let command = match direction {
                    FrameStepDirection::Backward => "frame-back-step",
                    FrameStepDirection::Forward => "frame-step",
                };
                mpv_error(mpv.command(command, &[]))?;
            }
        }
        Command::SetEqualizerEnabled(enabled) => {
            let next = options.equalizer.with_enabled(enabled);
            update_equalizer(mpv, tx, options, next)?;
        }
        Command::SetEqualizerPreset(preset) => {
            let next = options.equalizer.with_preset(preset);
            update_equalizer(mpv, tx, options, next)?;
        }
        Command::AdjustEqualizerPreamp(delta_db) => {
            let next = options.equalizer.adjusted_preamp(delta_db);
            update_equalizer(mpv, tx, options, next)?;
        }
        Command::AdjustEqualizerBand { index, delta_db } => {
            let Some(next) = options.equalizer.adjusted_band(index, delta_db) else {
                anyhow::bail!("Полоса эквалайзера {index} отсутствует");
            };
            update_equalizer(mpv, tx, options, next)?;
        }
        Command::ResetEqualizer => {
            let next = options.equalizer.reset();
            update_equalizer(mpv, tx, options, next)?;
        }
        Command::SetDarkTheme(dark_theme) => {
            mpv_error(mpv.set_property(
                "background-color",
                video_background_color(dark_theme).to_owned(),
            ))?;
        }
        Command::NextVideo => {
            if load_adjacent(mpv, tx, playlist, true, options.repeat_folder)? {
                options.media_available = true;
            }
        }
        Command::PreviousVideo => {
            let position = mpv.get_property::<f64>("time-pos").unwrap_or_default();
            if should_restart_current(position) {
                mpv_error(mpv.set_property("time-pos", 0.0))?;
                let _ = tx.send(Event::TimePos(0.0));
            } else if load_adjacent(mpv, tx, playlist, false, options.repeat_folder)? {
                options.media_available = true;
            }
        }
        Command::PlayIndex(index) => {
            let selected = playlist
                .as_mut()
                .and_then(|playlist| playlist.select(index));
            if selected.is_none() {
                anyhow::bail!("Медиафайл с индексом {index} отсутствует в плейлисте");
            }
            if load_selected_or_skip_missing(mpv, tx, playlist, true, false)? {
                options.media_available = true;
            }
        }
        Command::RemoveIndex(index) => {
            let playlist = playlist
                .as_mut()
                .context("Очередь воспроизведения ещё не создана")?;
            let removed_current = playlist
                .remove(index)
                .with_context(|| format!("Файл с индексом {index} отсутствует в очереди"))?;

            if playlist.current_path().is_none() {
                if options.media_available {
                    mpv_error(mpv.command("stop", &[]))?;
                }
                options.media_available = false;
                send_playlist(tx, playlist);
                let _ = tx.send(Event::MediaAvailable(false));
                let _ = tx.send(Event::PlaybackState(false));
                let _ = tx.send(Event::TimePos(0.0));
                let _ = tx.send(Event::PlaybackStopped);
            } else if removed_current {
                load_current(mpv, tx, playlist)?;
                options.media_available = true;
            } else {
                send_playlist(tx, playlist);
            }
        }
        Command::ToggleLoop => {
            let loop_file = mpv
                .get_property::<String>("loop-file")
                .unwrap_or_else(|_| "no".into());
            let value = if loop_value_is_enabled(&loop_file) {
                "no"
            } else {
                "inf"
            };
            mpv_error(mpv.set_property("loop-file", value.to_owned()))?;
        }
        Command::SetFolderLoop(enabled) => {
            options.repeat_folder = enabled;
            let _ = tx.send(Event::FolderLoopEnabled(options.repeat_folder));
        }
        Command::ToggleShuffle => {
            options.shuffled = !options.shuffled;
            if let Some(playlist) = playlist {
                playlist.set_shuffled(options.shuffled);
                send_playlist(tx, playlist);
            }
            let _ = tx.send(Event::ShuffleEnabled(options.shuffled));
        }
        Command::SetVideoWindow(handle) => {
            mpv_error(mpv.set_property("wid", handle.to_string()))
                .context("Не удалось привязать mpv к окну видео")?;
            mpv_error(mpv.set_property("vo", "gpu".to_owned()))
                .context("Не удалось выбрать видеовывод mpv")?;
            mpv_error(mpv.set_property("vid", "auto".to_owned()))
                .context("Не удалось включить видеодорожку")?;
        }
        Command::SeekRelative(offset) => {
            if options.media_available {
                mpv_error(mpv.command("seek", &[&offset.to_string(), "relative"]))?;
            }
        }
        Command::AdjustVolume(delta) => {
            let current = mpv.get_property::<f64>("volume").unwrap_or(50.0);
            let next = (current + delta).clamp(0.0, 100.0);
            if next > 0.0 {
                mpv_error(mpv.set_property("mute", false))?;
            }
            mpv_error(mpv.set_property("volume", next))?;
        }
        Command::ToggleMute => {
            let muted = mpv.get_property::<bool>("mute").unwrap_or(false);
            mpv_error(mpv.set_property("mute", !muted))?;
        }
        Command::RenderApiReady => {
            mpv_error(mpv.set_property("vid", "auto".to_owned()))
                .context("Не удалось включить видеодорожку")?;
        }
        Command::RequestMediaInfo => {
            if options.media_available {
                let info = MediaInfo::read(mpv);
                send_media_info(&info, tx);
            }
        }
        Command::ResolveResume(_) => {}
        Command::ClearPlaybackData => {}
        Command::Quit => return Ok(false),
    }
    Ok(true)
}

fn send_property_change(tx: &Sender<Event>, name: &str, change: PropertyData<'_>) {
    let event = match (name, change) {
        ("time-pos", PropertyData::Double(value)) => Some(Event::TimePos(value)),
        ("duration", PropertyData::Double(value)) => Some(Event::Duration(value)),
        ("pause", PropertyData::Flag(paused)) => Some(Event::PlaybackState(!paused)),
        ("volume", PropertyData::Double(value)) => Some(Event::Volume(value)),
        ("speed", PropertyData::Double(value)) => Some(Event::Speed(value)),
        ("mute", PropertyData::Flag(muted)) => Some(Event::Muted(muted)),
        ("loop-file", PropertyData::Str(value)) => {
            Some(Event::LoopEnabled(loop_value_is_enabled(value)))
        }
        ("audio-delay", PropertyData::Double(value)) => Some(Event::AudioDelay(value)),
        ("sub-delay", PropertyData::Double(value)) => Some(Event::SubtitleDelay(value)),
        ("chapter", PropertyData::Int64(value)) => Some(Event::ChapterChanged(value)),
        _ => None,
    };

    if let Some(event) = event {
        let _ = tx.send(event);
    }
}

pub fn run(rx: CommandReceiver, tx: Sender<Event>) -> anyhow::Result<()> {
    let mpv = create_mpv()?;
    let background_wake = Arc::clone(&mpv);
    let background_jobs = BackgroundJobs::new(move || background_wake.wakeup())?;
    let mut playlist: Option<DirectoryPlaylist> = None;
    let mut options = PlaybackOptions::default();
    let mut playback_diagnostics = PlaybackDiagnostics::from_environment();
    let mut session = SessionStore::load_default();
    if let Some(message) = session.take_storage_notice() {
        eprintln!("{message}");
        let _ = tx.send(Event::Error(message));
    }
    let mut last_session_save = Instant::now();
    let mut playback_state = PlaybackState::default();
    let mut events = mpv.create_event_context();
    // Windows starts with vid=no, so audio and metadata probing do not require
    // a render target. Media commands therefore do not wait for RenderApiReady;
    // a real video track asks the UI for one and enables vid=auto afterwards.

    let wake_mpv = Arc::clone(&mpv);
    rx.install_waker(move || wake_mpv.wakeup());

    let _ = tx.send(Event::RenderSource(MpvRenderSource::new(Arc::clone(&mpv))));
    send_equalizer(options.equalizer, &tx);
    send_recent_files(&tx, &session);

    mpv_error(events.disable_deprecated_events())?;
    observe_playback_properties(&mut events)?;
    playback_diagnostics.observe_properties(&mut events);

    loop {
        let mut dispatched_commands = 0;
        for _ in 0..DISPATCH_BUDGET {
            let Some(command) = rx.try_recv(true) else {
                break;
            };
            dispatched_commands += 1;
            let command = match command {
                Command::OpenFile(path) => {
                    background_jobs.cancel_all();
                    match DirectoryPlaylist::from_file(&path).and_then(|next_playlist| {
                        activate_playlist(&mpv, &tx, &mut playlist, next_playlist, options.shuffled)
                    }) {
                        Ok(()) => {
                            options.media_available = true;
                            let current_path = playlist
                                .as_ref()
                                .and_then(DirectoryPlaylist::current_path)
                                .cloned();
                            playback_state.begin_media_change(current_path);
                            background_jobs.expand_file(playback_state.generation(), path);
                        }
                        Err(error) => {
                            options.media_available = false;
                            playback_state.media_change_failed();
                            send_playback_failed(&tx, error.to_string());
                        }
                    }
                    continue;
                }
                Command::OpenFolder(path) => {
                    background_jobs.open_directory(path);
                    continue;
                }
                command => command,
            };
            if let Command::ResolveResume(continue_saved) = &command {
                let resolution = (|| -> anyhow::Result<()> {
                    let Some((path, position)) = playback_state.take_resume_choice() else {
                        return Ok(());
                    };
                    let current_path = playlist.as_ref().and_then(DirectoryPlaylist::current_path);
                    if current_path != Some(&path) {
                        return Ok(());
                    }

                    if *continue_saved {
                        mpv_error(mpv.set_property("time-pos", position))?;
                        let _ = tx.send(Event::TimePos(position));
                    } else {
                        session.clear_position(&path);
                        save_session(&tx, &mut session);
                        mpv_error(mpv.set_property("time-pos", 0.0))?;
                        let _ = tx.send(Event::TimePos(0.0));
                    }
                    mpv_error(mpv.set_property("pause", false))?;
                    Ok(())
                })();
                if let Err(error) = resolution {
                    let _ = tx.send(Event::Error(format!(
                        "Не удалось применить выбор продолжения: {error:#}"
                    )));
                }
                continue;
            }

            if matches!(&command, Command::ClearPlaybackData) {
                playback_state.suppress_position_recording_for(
                    playlist
                        .as_ref()
                        .and_then(DirectoryPlaylist::current_path)
                        .cloned(),
                );
                match session.clear_playback_data() {
                    Ok(()) => {
                        send_recent_files(&tx, &session);
                        let _ = tx.send(Event::PlaybackDataCleared);
                    }
                    Err(error) => {
                        eprintln!("Не удалось очистить историю и позиции: {error}");
                        let _ = tx.send(Event::Error(format!(
                            "Не удалось очистить историю и позиции: {error}"
                        )));
                    }
                }
                continue;
            }

            let previous_media_path = playlist
                .as_ref()
                .and_then(DirectoryPlaylist::current_path)
                .cloned();
            let reloads_selected_file = matches!(&command, Command::PlayIndex(_));
            let changes_media = matches!(
                &command,
                Command::NextVideo
                    | Command::PreviousVideo
                    | Command::PlayIndex(_)
                    | Command::RemoveIndex(_)
            );
            if changes_media || matches!(&command, Command::Stop) {
                background_jobs.cancel_playlist();
            }
            if matches!(&command, Command::Stop) {
                playback_state.cancel_resume();
                if let Some(path) = playlist.as_ref().and_then(DirectoryPlaylist::current_path) {
                    session.clear_position(path);
                    save_session(&tx, &mut session);
                }
            }
            if matches!(&command, Command::Quit) {
                flush_session(&tx, &mut session);
            }

            match handle_command(command, &mpv, &tx, &mut playlist, &mut options) {
                Ok(true) => {
                    if changes_media {
                        let current_media_path = playlist
                            .as_ref()
                            .and_then(DirectoryPlaylist::current_path)
                            .cloned();
                        let expected_resume_path = (reloads_selected_file
                            || current_media_path != previous_media_path)
                            .then_some(current_media_path)
                            .flatten();
                        if expected_resume_path.is_some() {
                            background_jobs.cancel_metadata();
                        }
                        playback_state.begin_media_change(expected_resume_path);
                    }
                }
                Ok(false) => {
                    if let Err(error) = playback_diagnostics.write_report(options.media_available) {
                        eprintln!("Не удалось сохранить диагностику воспроизведения: {error:#}");
                    }
                    return Ok(());
                }
                Err(error) => {
                    eprintln!("Ошибка команды mpv: {error:#}");
                    if changes_media {
                        background_jobs.cancel_metadata();
                        playback_state.media_change_failed();
                        options.media_available = false;
                        send_playback_failed(&tx, error.to_string());
                    } else {
                        let _ = tx.send(Event::Error(error.to_string()));
                    }
                }
            }
        }

        let current_path = playlist
            .as_ref()
            .and_then(DirectoryPlaylist::current_path)
            .map(|path| path.as_path());
        let ready_background =
            background_jobs.take_ready(playback_state.generation(), current_path);
        if let Some(result) = ready_background.playlist {
            match result {
                ReadyPlaylist::FileExpansion(result) => match result {
                    Ok(next_playlist) => replace_playlist_without_reload(
                        &tx,
                        &mut playlist,
                        next_playlist,
                        options.shuffled,
                    ),
                    Err(error) => {
                        let _ = tx.send(Event::Error(bounded_text(
                            &format!("Не удалось дополнить очередь: {error:#}"),
                            UI_TEXT_BYTES,
                        )));
                    }
                },
                ReadyPlaylist::Directory(result) => match result.and_then(|next_playlist| {
                    activate_playlist(&mpv, &tx, &mut playlist, next_playlist, options.shuffled)
                }) {
                    Ok(()) => {
                        background_jobs.cancel_metadata();
                        options.media_available = true;
                        playback_state.begin_media_change(
                            playlist
                                .as_ref()
                                .and_then(DirectoryPlaylist::current_path)
                                .cloned(),
                        );
                    }
                    Err(error) => {
                        background_jobs.cancel_metadata();
                        options.media_available = false;
                        playback_state.media_change_failed();
                        send_playback_failed(&tx, error.to_string());
                    }
                },
            }
        }
        if let Some(result) = ready_background.metadata {
            send_audio_metadata(result, &tx);
        }

        // With no command to dispatch, block until mpv has work, a producer
        // calls mpv_wakeup(), or the next periodic session save is due. Further
        // queued events are drained without blocking so they cannot starve
        // commands or Quit.
        let blocking_wait_seconds = if dispatched_commands == 0 {
            SESSION_SAVE_INTERVAL
                .saturating_sub(last_session_save.elapsed())
                .as_secs_f64()
        } else {
            0.0
        };
        for event_index in 0..64 {
            if rx.quit_requested() {
                break;
            }
            let timeout = if event_index == 0 {
                blocking_wait_seconds
            } else {
                0.0
            };
            let Some(event) = events.wait_event(timeout) else {
                break;
            };
            match event {
                Ok(MpvEvent::StartFile) => {
                    playback_diagnostics.reset_for_new_media();
                }
                Ok(MpvEvent::FileLoaded) if options.media_available => {
                    let mut resume_position = None;
                    if let Some(path) = playlist.as_ref().and_then(DirectoryPlaylist::current_path)
                    {
                        playback_state.file_loaded(path);
                        let duration = mpv.get_property::<f64>("duration").unwrap_or_default();
                        if let Some(position) = session.resume_position(path, duration)
                            && set_loaded_media_paused(&mpv, true).is_ok()
                        {
                            playback_state.offer_resume(path.clone(), position);
                            resume_position = Some(position);
                        }
                    }
                    if resume_position.is_none() {
                        playback_state.finish_loading_without_resume();
                        // pause is a persistent mpv property and can survive a
                        // stopped, paused, or completed item. A newly selected
                        // file must autoplay unless Oxide is deliberately
                        // holding it for the resume-position prompt above.
                        if let Err(error) = set_loaded_media_paused(&mpv, false) {
                            let _ = tx.send(Event::Error(format!(
                                "Не удалось начать воспроизведение: {error:#}"
                            )));
                        }
                    }
                    if let Some(path) = playlist.as_ref().and_then(DirectoryPlaylist::current_path)
                        && playback_state.should_record_media(path)
                    {
                        record_current_media(&tx, &playlist, &mut session);
                    }
                    let info = send_media_ready(&mpv, &tx);
                    // Tag and artwork probing can require real HDD reads. Video
                    // never consumes this presentation data, so avoid delaying
                    // its FileLoaded/VideoReconfig path. For audio, readiness is
                    // already visible before the bounded metadata read starts.
                    if audio_metadata_is_relevant(&info)
                        && let Some(path) =
                            playlist.as_ref().and_then(DirectoryPlaylist::current_path)
                    {
                        background_jobs.request_metadata(playback_state.generation(), path.clone());
                    }
                    if let Some(position) = resume_position {
                        let _ = tx.send(Event::ResumeAvailable(position));
                    }
                }
                Ok(MpvEvent::EndFile(reason)) if reason == mpv_end_file_reason::Eof => {
                    if let Some(path) = playlist.as_ref().and_then(DirectoryPlaylist::current_path)
                    {
                        session.clear_position(path);
                    }
                    match load_adjacent(&mpv, &tx, &mut playlist, true, options.repeat_folder) {
                        Ok(true) => {
                            options.media_available = true;
                            background_jobs.cancel_metadata();
                            playback_state.expect_resume_for(
                                playlist
                                    .as_ref()
                                    .and_then(DirectoryPlaylist::current_path)
                                    .cloned(),
                            );
                        }
                        Ok(false) => {
                            options.media_available = false;
                            send_playback_finished(&tx);
                        }
                        Err(error) => {
                            options.media_available = false;
                            send_playback_failed(&tx, error.to_string());
                        }
                    }
                }
                Ok(MpvEvent::EndFile(reason)) if reason == mpv_end_file_reason::Error => {
                    match skip_failed_current(&mpv, &tx, &mut playlist, options.repeat_folder) {
                        Ok(true) => {
                            options.media_available = true;
                            background_jobs.cancel_metadata();
                            playback_state.expect_resume_for(
                                playlist
                                    .as_ref()
                                    .and_then(DirectoryPlaylist::current_path)
                                    .cloned(),
                            );
                        }
                        Ok(false) => {
                            options.media_available = false;
                            send_playback_failed(
                                &tx,
                                "Не удалось воспроизвести медиафайл; очередь завершена".to_owned(),
                            );
                        }
                        Err(error) => {
                            options.media_available = false;
                            send_playback_failed(&tx, error.to_string());
                        }
                    }
                }
                Ok(MpvEvent::VideoReconfig) if options.media_available => {
                    let _ = send_media_ready(&mpv, &tx);
                    let _ = tx.send(Event::VideoOutputReady);
                }
                Ok(MpvEvent::AudioReconfig) if options.media_available => {
                    let _ = send_media_ready(&mpv, &tx);
                }
                Ok(MpvEvent::PropertyChange { name, change, .. }) => {
                    playback_diagnostics.update(name, &change);
                    if name == "time-pos" {
                        let position = mpv.get_property::<f64>("time-pos").unwrap_or_default();
                        let duration = mpv.get_property::<f64>("duration").unwrap_or_default();
                        if let Some(path) =
                            playlist.as_ref().and_then(DirectoryPlaylist::current_path)
                            && playback_state.should_record_position(path)
                        {
                            session.record_position(path, position, duration);
                        }
                    }
                    send_property_change(&tx, name, change);
                }
                Ok(MpvEvent::QueueOverflow) => {
                    let _ = tx.send(Event::Error("Очередь событий mpv переполнена".to_owned()));
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = tx.send(Event::Error(format!("Ошибка mpv: {error:?}")));
                }
            }
        }

        if last_session_save.elapsed() >= SESSION_SAVE_INTERVAL {
            save_session(&tx, &mut session);
            last_session_save = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        adjusted_delay, audio_metadata_is_relevant, loop_value_is_enabled, quote_mpv_command_arg,
        should_restart_current,
    };
    use crate::ipc::AspectRatio;
    use crate::player::media_info::MediaInfo;

    #[test]
    fn maps_aspect_ratio_presets_to_mpv_values() {
        assert_eq!(AspectRatio::Original.mpv_value(), "no");
        assert_eq!(AspectRatio::Wide.mpv_value(), "16:9");
        assert_eq!(AspectRatio::Standard.mpv_value(), "4:3");
        assert_eq!(AspectRatio::Flat.mpv_value(), "1.85:1");
        assert_eq!(AspectRatio::Scope.mpv_value(), "2.39:1");
    }

    #[test]
    fn recognizes_normalized_mpv_loop_values() {
        assert!(loop_value_is_enabled("inf"));
        assert!(loop_value_is_enabled("yes"));
        assert!(!loop_value_is_enabled("no"));
        assert!(!loop_value_is_enabled("0"));
    }

    #[test]
    fn previous_restarts_only_after_the_opening_threshold() {
        assert!(!should_restart_current(0.0));
        assert!(!should_restart_current(3.0));
        assert!(should_restart_current(3.01));
        assert!(!should_restart_current(f64::NAN));
    }

    #[test]
    fn quotes_windows_paths_for_mpv_command_strings() {
        assert_eq!(
            quote_mpv_command_arg(r#"Z:\Media\Director's "cut"\captions.srt"#),
            r#""Z:\\Media\\Director's \"cut\"\\captions.srt""#
        );
    }

    #[test]
    fn adjusts_sync_delay_in_tenths_and_clamps_extremes() {
        assert_eq!(adjusted_delay(0.0, 0.1), 0.1);
        assert_eq!(adjusted_delay(0.3, -0.1), 0.2);
        assert_eq!(adjusted_delay(29.9, 1.0), 30.0);
        assert_eq!(adjusted_delay(-29.9, -1.0), -30.0);
    }

    #[test]
    fn expensive_tag_reads_are_reserved_for_actual_audio_only_media() {
        assert!(audio_metadata_is_relevant(&MediaInfo {
            audio_track_count: 1,
            ..MediaInfo::default()
        }));
        assert!(!audio_metadata_is_relevant(&MediaInfo {
            video_track_count: 1,
            audio_track_count: 1,
            ..MediaInfo::default()
        }));
        assert!(!audio_metadata_is_relevant(&MediaInfo::default()));
    }
}
