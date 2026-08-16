use std::{path::Path, path::PathBuf, time::Duration};

use crate::ipc::{Command, Event};
use crate::player::{media_info::MediaInfo, playlist::DirectoryPlaylist};
use crossbeam_channel::{Receiver, Sender};
use libmpv::{
    events::{Event as MpvEvent, PropertyData},
    mpv_end_file_reason, Format, Mpv,
};

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
) -> anyhow::Result<()> {
    let next_playlist = DirectoryPlaylist::load_from_path(&path)?;
    load_file_in_mpv(mpv, &path)?;

    if let Some(file_name) = path.file_name() {
        let _ = tx.send(Event::FileLoaded(file_name.to_string_lossy().into_owned()));
    }
    send_playlist(tx, &next_playlist);
    let _ = tx.send(Event::MediaAvailable(true));
    *playlist = Some(next_playlist);
    Ok(())
}

fn load_adjacent(
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
    next: bool,
) -> anyhow::Result<bool> {
    let path = playlist.as_ref().and_then(|playlist| {
        if next {
            playlist.next_path()
        } else {
            playlist.previous_path()
        }
    });

    if let Some(path) = path.cloned() {
        load_path(mpv, tx, playlist, path)?;
        return Ok(true);
    }
    Ok(false)
}

fn handle_command(
    command: Command,
    mpv: &Mpv,
    tx: &Sender<Event>,
    playlist: &mut Option<DirectoryPlaylist>,
) -> anyhow::Result<bool> {
    match command {
        Command::OpenFile(path) => {
            load_path(mpv, tx, playlist, PathBuf::from(path))?;
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
            load_adjacent(mpv, tx, playlist, true)?;
        }
        Command::PreviousVideo => {
            load_adjacent(mpv, tx, playlist, false)?;
        }
        Command::ToggleLoop => {
            let loop_file = mpv
                .get_property::<String>("loop-file")
                .unwrap_or_else(|_| "no".into());
            let value = if loop_file == "yes" { "no" } else { "yes" };
            mpv_error(mpv.set_property("loop-file", value.to_owned()))?;
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
            match handle_command(command, &mpv, &tx, &mut playlist) {
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
                    match load_adjacent(&mpv, &tx, &mut playlist, true) {
                        Ok(true) => {}
                        Ok(false) => {
                            let _ = tx.send(Event::MediaAvailable(false));
                            let _ = tx.send(Event::PlaybackState(false));
                        }
                        Err(error) => {
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
