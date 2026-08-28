use crate::render::mpv_opengl::MpvRenderSource;

#[derive(Debug)]
pub enum Command {
    OpenFile(String),
    OpenFolder(String),

    TogglePlay,

    Stop,

    Seek(f64),

    SetVolume(f64),
    SetSpeed(f64),

    NextVideo,
    PreviousVideo,
    PlayIndex(usize),

    ToggleLoop,
    ToggleFolderLoop,
    ToggleShuffle,

    SeekRelative(f64),
    AdjustVolume(f64),
    ToggleMute,

    RenderApiReady,
    SetVideoWindow(usize),

    RequestMediaInfo,

    Quit,
}

#[derive(Debug)]
pub enum Event {
    RenderSource(MpvRenderSource),
    TimePos(f64),
    Duration(f64),

    PlaybackState(bool),
    LoadStarted(String),
    PlaybackStopped,
    PlaybackFinished,
    PlaybackFailed(String),

    Volume(f64),
    Muted(bool),
    Speed(f64),

    FileLoaded(String),
    MediaAvailable(bool),
    VideoOutputReady,

    Playlist {
        items: Vec<String>,
        current_index: usize,
    },

    MediaInfo {
        width: u32,
        height: u32,
        fps: f64,
        codec: String,
        audio_codec: String,
    },

    LoopEnabled(bool),
    FolderLoopEnabled(bool),
    ShuffleEnabled(bool),

    Error(String),
}
