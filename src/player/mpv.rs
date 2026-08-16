use std::{path::Path, path::PathBuf, time::Duration};

use crate::ipc::{Command, Event};
use crate::player::{media_info::MediaInfo, playlist::DirectoryPlaylist};
use anyhow::Context;
use crossbeam_channel::{Receiver, Sender};
use libmpv::{
    events::{Event as MpvEvent, PropertyData},
    mpv_end_file_reason, Format, Mpv,
};

#[derive(Default)]
struct PlaybackOptions {
    repeat_folder: bool,
    shuffled: bool,
}

fn mpv_error<T>(result: Result<T, libmpv::Error>) -> anyhow::Result<T> {
    result.map_err(|e| anyhow::anyhow!("{:?}", e))
}

fn send_playlist(tx: &Sender<Event>, playlist: &DirectoryPlaylist) {
    let _ = tx.send(Event::Playlist {
        items: playlist.items(),
        current_index: playlist.current_index(),
    });
}

fn send_media_info(mpv: &Mpv, tx: &Sender<Event>) {
    let info = MediaInfo::read(mpv);

    let _ = tx.send(Event::MediaInfo {
        width: info.width,
        height: info.height,
        fps: info.fps,
        codec: info.codec,
        audio_codec: info.audio_codec,
    });
}

fn load_file_in_mpv(mpv: &Mpv, path: &Path) -> anyhow::Result<()> {
    mpv_error(mpv.command("loadfile", &[path.to_string_lossy().as_ref()]))
}

fn load_path(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    path: PathBuf,
    shuffled: bool,
) -> anyhow::Result<()> {
    let next_playlist = DirectoryPlaylist::load_from_path(&path)?;
    activate_playlist(mpv, tx, playlist, next_playlist, shuffled)
}

fn load_directory(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    directory: &Path,
    shuffled: bool,
) -> anyhow::Result<()> {
    let next_playlist = DirectoryPlaylist::load_from_directory(directory)?;
    activate_playlist(mpv, tx, playlist, next_playlist, shuffled)
}

fn activate_playlist(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    mut next_playlist: DirectoryPlaylist,
    shuffled: bool,
) -> anyhow::Result<()> {
    next_playlist.set_shuffled(shuffled);
    load_current(mpv, tx, &next_playlist)?;
    *playlist = Some(next_playlist);
    Ok(())
}

fn load_current(mpv: &Mpv, tx: &Sender<Event>, playlist: &DirectoryPlaylist) -> anyhow::Result<()> {
    let path = playlist
        .current_path()
        .context("В плейлисте нет видео для воспроизведения")?;
    load_file_in_mpv(mpv, path)?;

    if let Some(file_name) = path.file_name() {
        let _ = tx.send(Event::FileLoaded(file_name.to_string_lossy().into_owned()));
    }
    send_playlist(tx, playlist);
    let _ = tx.send(Event::MediaAvailable(true));
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
}

fn handle_command(
    command: Command,
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    options: &mut PlaybackOptions,
) -> anyhow::Result<bool> {
    match command {
        Command::OpenFile(path) => {
            load_path(mpv, tx, playlist, PathBuf::from(path), options.shuffled)?;
        }
        Command::OpenFolder(path) => {
            load_directory(mpv, tx, playlist, Path::new(&path), options.shuffled)?;
        }
        Command::TogglePlay => {
            let paused = mpv.get_property::<bool>("pause").unwrap_or(false);
            mpv_error(mpv.set_property("pause", !paused))?;
        }
        Command::Stop => {
            mpv_error(mpv.command("stop", &[]))?;
            let _ = tx.send(Event::MediaAvailable(false));
            let _ = tx.send(Event::PlaybackState(false));
            let _ = tx.send(Event::TimePos(0.0));
        }
        Command::Seek(position) => {
            mpv_error(mpv.set_property("time-pos", position.max(0.0)))?;
        }
        Command::SetVolume(value) => {
            mpv_error(mpv.set_property("volume", value.clamp(0.0, 100.0)))?;
        }
        Command::SetSpeed(value) => {
            mpv_error(mpv.set_property("speed", value.clamp(0.25, 4.0)))?;
        }
        Command::NextVideo => {
            load_adjacent(mpv, tx, playlist, true, options.repeat_folder)?;
        }
        Command::PreviousVideo => {
            load_adjacent(mpv, tx, playlist, false, options.repeat_folder)?;
        }
        Command::ToggleLoop => {
            let loop_file = mpv
                .get_property::<String>("loop-file")
                .unwrap_or_else(|_| "no".into());
            let value = if loop_file == "yes" { "no" } else { "yes" };
            mpv_error(mpv.set_property("loop-file", value.to_owned()))?;
        }
        Command::ToggleFolderLoop => {
            options.repeat_folder = !options.repeat_folder;
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
            mpv_error(mpv.set_property("wid", handle as i64))?;
            mpv_error(mpv.set_property("vo", "gpu".to_owned()))?;
            mpv_error(mpv.set_property("gpu-context", "win32".to_owned()))?;
        }
        Command::SeekRelative(offset) => {
            mpv_error(mpv.command("seek", &[&offset.to_string(), "relative"]))?;
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
        Command::RequestMediaInfo => send_media_info(mpv, tx),
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
        ("loop-file", PropertyData::Str(value)) => Some(Event::LoopEnabled(value == "yes")),
        _ => None,
    };

    if let Some(event) = event {
        let _ = tx.send(event);
    }
}

pub fn run(rx: Receiver<Command>, tx: Sender<Event>) -> anyhow::Result<()> {
    let mpv = Mpv::new().map_err(|e| anyhow::anyhow!("{:?}", e))?;
    let mut playlist: Option<DirectoryPlaylist> = None;
    let mut options = PlaybackOptions::default();
    let mut events = mpv.create_event_context();

    mpv_error(events.disable_deprecated_events())?;
    for (id, name, format) in [
        (1, "time-pos", Format::Double),
        (2, "duration", Format::Double),
        (3, "pause", Format::Flag),
        (4, "volume", Format::Double),
        (5, "speed", Format::Double),
        (6, "mute", Format::Flag),
        (7, "loop-file", Format::String),
    ] {
        mpv_error(events.observe_property(name, format, id))?;
    }

    loop {
        while let Ok(command) = rx.try_recv() {
            match handle_command(command, &mpv, &tx, &mut playlist, &mut options) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => {
                    let _ = tx.send(Event::Error(error.to_string()));
                }
            }
        }

        while let Some(event) = events.wait_event(0.0) {
            match event {
                Ok(MpvEvent::FileLoaded) => send_media_info(&mpv, &tx),
                Ok(MpvEvent::EndFile(reason)) if reason == mpv_end_file_reason::Eof => {
                    match load_adjacent(&mpv, &tx, &mut playlist, true, options.repeat_folder) {
                        Ok(true) => {}
                        Ok(false) => send_playback_finished(&tx),
                        Err(error) => {
                            let _ = tx.send(Event::Error(error.to_string()));
                        }
                    }
                }
                Ok(MpvEvent::EndFile(reason)) if reason == mpv_end_file_reason::Error => {
                    match skip_failed_current(&mpv, &tx, &mut playlist, options.repeat_folder) {
                        Ok(true) => {}
                        Ok(false) => {
                            send_playback_finished(&tx);
                            let _ = tx.send(Event::Error(
                                "Не удалось воспроизвести видео; очередь завершена".to_owned(),
                            ));
                        }
                        Err(error) => {
                            send_playback_finished(&tx);
                            let _ = tx.send(Event::Error(error.to_string()));
                        }
                    }
                }
                Ok(MpvEvent::VideoReconfig | MpvEvent::AudioReconfig) => {
                    send_media_info(&mpv, &tx);
                }
                Ok(MpvEvent::PropertyChange { name, change, .. }) => {
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

        std::thread::sleep(Duration::from_millis(5));
    }
}
