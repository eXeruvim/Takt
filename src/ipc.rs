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

    ToggleLoop,

    SeekRelative(f64),
    AdjustVolume(f64),
    ToggleMute,

    SetVideoWindow(usize),

    RequestMediaInfo,

    Quit,
}

#[derive(Debug)]
pub enum Event {
    TimePos(f64),
    Duration(f64),

    PlaybackState(bool),

    Volume(f64),
    Muted(bool),
    Speed(f64),

    FileLoaded(String),
    MediaAvailable(bool),

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

    Error(String),
}
