use crate::render::mpv_opengl::MpvRenderSource;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AspectRatio {
    #[default]
    Original,
    Wide,
    Standard,
    Flat,
    Scope,
}

impl AspectRatio {
    pub const fn from_ui_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Original),
            1 => Some(Self::Wide),
            2 => Some(Self::Standard),
            3 => Some(Self::Flat),
            4 => Some(Self::Scope),
            _ => None,
        }
    }

    pub const fn ui_index(self) -> i32 {
        match self {
            Self::Original => 0,
            Self::Wide => 1,
            Self::Standard => 2,
            Self::Flat => 3,
            Self::Scope => 4,
        }
    }

    pub const fn mpv_value(self) -> &'static str {
        match self {
            Self::Original => "no",
            Self::Wide => "16:9",
            Self::Standard => "4:3",
            Self::Flat => "1.85:1",
            Self::Scope => "2.39:1",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameStepDirection {
    Backward,
    Forward,
}

#[derive(Debug)]
pub enum Command {
    OpenFile(String),
    OpenFolder(String),

    TogglePlay,

    Stop,

    Seek(f64),

    SetVolume(f64),
    SetSpeed(f64),
    SelectAudioTrack(i64),
    SelectSubtitleTrack(Option<i64>),
    LoadSubtitle(String),
    AdjustAudioDelay(f64),
    AdjustSubtitleDelay(f64),
    ResetAudioDelay,
    ResetSubtitleDelay,
    SelectChapter(i64),
    SetAspectRatio(AspectRatio),
    SetHardwareDecoding(bool),
    StepFrame(FrameStepDirection),
    SetDarkTheme(bool),

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioTrackInfo {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleTrackInfo {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterInfo {
    pub index: i64,
    pub label: String,
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
    AudioDelay(f64),
    SubtitleDelay(f64),

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
    AudioMetadata {
        title: String,
        artist: String,
        album: String,
        artwork_rgba: Vec<u8>,
        artwork_width: u32,
        artwork_height: u32,
    },
    AudioTracks(Vec<AudioTrackInfo>),
    SubtitleTracks(Vec<SubtitleTrackInfo>),
    Chapters {
        items: Vec<ChapterInfo>,
        current_index: i64,
    },
    ChapterChanged(i64),
    AspectRatioChanged(AspectRatio),
    HardwareDecoding {
        enabled: bool,
        decoder: String,
    },

    LoopEnabled(bool),
    FolderLoopEnabled(bool),
    ShuffleEnabled(bool),

    Error(String),
}
