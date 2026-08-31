#[cfg(target_os = "windows")]
use std::time::Duration;
use std::{
    path::Path,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::ipc::{AspectRatio, Command, EqualizerPreset, Event, FrameStepDirection};
use crate::player::playlist::{
    AUDIO_EXTENSIONS, MEDIA_EXTENSIONS, VIDEO_EXTENSIONS, is_media_file,
};
#[cfg(target_os = "windows")]
use crate::render::mpv_opengl::{MpvOpenGlRenderer, MpvRenderSource};
#[cfg(target_os = "windows")]
use crate::render::mpv_render::VideoHost;
#[cfg(target_os = "windows")]
use crate::ui::native_fullscreen::NativeFullscreen;
use slint::winit_030::{
    EventResult, WinitWindowAccessor,
    winit::{
        dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
        event::{MouseButton, WindowEvent},
        keyboard::{KeyCode, ModifiersState, PhysicalKey},
        window::Theme as WinitTheme,
    },
};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{RDW_INTERNALPAINT, RedrawWindow},
    UI::{
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
        WindowsAndMessaging::{GetCursorPos, GetWindowPlacement, WINDOWPLACEMENT},
    },
};

slint::include_modules!();

#[cfg(target_os = "windows")]
const DRAWER_CLOSE_DELAY: Duration = Duration::from_millis(280);

const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"];

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Default)]
struct WindowDragState {
    active: bool,
    cursor_origin: (i32, i32),
    window_origin: (i32, i32),
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Default)]
struct PictureInPictureState {
    active: bool,
    saved_position: Option<PhysicalPosition<i32>>,
    saved_size: Option<PhysicalSize<u32>>,
    was_maximized: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i32)]
enum ShortcutAction {
    OpenFile = 0,
    OpenFolder = 1,
    PlayPause = 2,
    Previous = 3,
    Next = 4,
    ToggleMute = 5,
    ToggleLoop = 6,
    TogglePlaylist = 7,
    ToggleInfo = 8,
    ToggleShortcuts = 10,
    ToggleFullscreen = 11,
    Stop = 12,
    SeekBackwardLong = 13,
    SeekForwardLong = 14,
    VolumeDown = 15,
    VolumeUp = 16,
    SeekBackwardShort = 22,
    SeekForwardShort = 23,
    ToggleCommandPalette = 24,
    FrameBackward = 25,
    FrameForward = 26,
    TogglePictureInPicture = 27,
}

#[derive(Clone, Copy)]
enum DrawerSwipeTarget {
    Info,
    Playlist,
    Settings,
}

impl ShortcutAction {
    const ALL: [Self; 22] = [
        Self::ToggleCommandPalette,
        Self::OpenFile,
        Self::OpenFolder,
        Self::TogglePlaylist,
        Self::PlayPause,
        Self::Stop,
        Self::Previous,
        Self::Next,
        Self::SeekBackwardLong,
        Self::SeekForwardLong,
        Self::SeekBackwardShort,
        Self::SeekForwardShort,
        Self::FrameBackward,
        Self::FrameForward,
        Self::VolumeDown,
        Self::VolumeUp,
        Self::ToggleMute,
        Self::ToggleLoop,
        Self::ToggleFullscreen,
        Self::TogglePictureInPicture,
        Self::ToggleInfo,
        Self::ToggleShortcuts,
    ];

    fn from_id(id: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|action| *action as i32 == id)
    }

    fn from_palette_action(action: i32) -> Option<Self> {
        match action {
            0 => Some(Self::OpenFile),
            1 => Some(Self::OpenFolder),
            2 => Some(Self::PlayPause),
            3 => Some(Self::Previous),
            4 => Some(Self::Next),
            5 => Some(Self::ToggleMute),
            6 => Some(Self::ToggleLoop),
            7 => Some(Self::TogglePlaylist),
            8 => Some(Self::ToggleInfo),
            10 => Some(Self::ToggleShortcuts),
            11 => Some(Self::ToggleFullscreen),
            12 => Some(Self::Stop),
            13 => Some(Self::SeekBackwardLong),
            14 => Some(Self::SeekForwardLong),
            15 => Some(Self::VolumeDown),
            16 => Some(Self::VolumeUp),
            25 => Some(Self::FrameBackward),
            26 => Some(Self::FrameForward),
            27 => Some(Self::TogglePictureInPicture),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::OpenFile => "Открыть файл",
            Self::OpenFolder => "Открыть папку",
            Self::PlayPause => "Пауза / воспроизведение",
            Self::Stop => "Остановить воспроизведение",
            Self::Previous => "Предыдущий файл",
            Self::Next => "Следующий файл",
            Self::SeekBackwardShort => "Назад на 5 секунд",
            Self::SeekForwardShort => "Вперёд на 5 секунд",
            Self::SeekBackwardLong => "Назад на 10 секунд",
            Self::SeekForwardLong => "Вперёд на 10 секунд",
            Self::FrameBackward => "Предыдущий кадр",
            Self::FrameForward => "Следующий кадр",
            Self::VolumeDown => "Уменьшить громкость",
            Self::VolumeUp => "Увеличить громкость",
            Self::ToggleMute => "Выключить / включить звук",
            Self::ToggleLoop => "Повтор файла",
            Self::ToggleFullscreen => "Полноэкранный режим",
            Self::TogglePictureInPicture => "Картинка в картинке",
            Self::ToggleInfo => "Свойства файла",
            Self::TogglePlaylist => "Плейлист",
            Self::ToggleShortcuts => "Горячие клавиши",
            Self::ToggleCommandPalette => "Палитра команд",
        }
    }

    fn default_bindings(self) -> Vec<ShortcutBinding> {
        let plain = |code| ShortcutBinding::new(code, ModifiersState::empty());
        let control = |code| ShortcutBinding::new(code, ModifiersState::CONTROL);

        match self {
            Self::OpenFile => vec![control(KeyCode::KeyO), plain(KeyCode::KeyO)],
            Self::OpenFolder => vec![ShortcutBinding::new(
                KeyCode::KeyO,
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            )],
            Self::PlayPause => vec![plain(KeyCode::Space), plain(KeyCode::KeyK)],
            Self::Stop => vec![plain(KeyCode::KeyS)],
            Self::Previous => vec![plain(KeyCode::KeyP)],
            Self::Next => vec![plain(KeyCode::KeyN)],
            Self::SeekBackwardShort => vec![plain(KeyCode::ArrowLeft)],
            Self::SeekForwardShort => vec![plain(KeyCode::ArrowRight)],
            Self::SeekBackwardLong => vec![plain(KeyCode::KeyJ)],
            Self::SeekForwardLong => vec![plain(KeyCode::KeyL)],
            Self::FrameBackward => vec![plain(KeyCode::Comma)],
            Self::FrameForward => vec![plain(KeyCode::Period)],
            Self::VolumeDown => vec![plain(KeyCode::ArrowDown)],
            Self::VolumeUp => vec![plain(KeyCode::ArrowUp)],
            Self::ToggleMute => vec![plain(KeyCode::KeyM)],
            Self::ToggleLoop => vec![plain(KeyCode::KeyR)],
            Self::ToggleFullscreen => vec![plain(KeyCode::KeyF)],
            Self::TogglePictureInPicture => {
                vec![ShortcutBinding::new(KeyCode::KeyP, ModifiersState::ALT)]
            }
            Self::ToggleInfo => vec![plain(KeyCode::KeyI)],
            Self::TogglePlaylist => vec![control(KeyCode::KeyL)],
            Self::ToggleShortcuts => vec![plain(KeyCode::KeyH)],
            Self::ToggleCommandPalette => vec![control(KeyCode::KeyK)],
        }
    }

    fn allows_repeat(self) -> bool {
        matches!(
            self,
            Self::SeekBackwardShort
                | Self::SeekForwardShort
                | Self::SeekBackwardLong
                | Self::SeekForwardLong
                | Self::VolumeDown
                | Self::VolumeUp
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShortcutBinding {
    code: KeyCode,
    modifiers: ModifiersState,
}

impl ShortcutBinding {
    fn new(code: KeyCode, modifiers: ModifiersState) -> Self {
        Self { code, modifiers }
    }
}

#[derive(Debug)]
struct ShortcutConflict {
    action: ShortcutAction,
    binding: ShortcutBinding,
}

#[derive(Clone, Debug)]
struct ShortcutRegistry {
    bindings: Vec<(ShortcutAction, Vec<ShortcutBinding>)>,
}

impl Default for ShortcutRegistry {
    fn default() -> Self {
        Self {
            bindings: ShortcutAction::ALL
                .into_iter()
                .map(|action| (action, action.default_bindings()))
                .collect(),
        }
    }
}

impl ShortcutRegistry {
    fn action_for(&self, binding: ShortcutBinding) -> Option<ShortcutAction> {
        self.bindings
            .iter()
            .find_map(|(action, bindings)| bindings.contains(&binding).then_some(*action))
    }

    fn display_for(&self, action: ShortcutAction) -> String {
        self.bindings
            .iter()
            .find(|(candidate, _)| *candidate == action)
            .map(|(_, bindings)| {
                bindings
                    .iter()
                    .map(|binding| binding.display())
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_else(|| "—".to_string())
    }

    fn rebind(
        &mut self,
        action: ShortcutAction,
        binding: ShortcutBinding,
    ) -> Result<(), ShortcutConflict> {
        if let Some(conflicting_action) = self.action_for(binding)
            && conflicting_action != action
        {
            return Err(ShortcutConflict {
                action: conflicting_action,
                binding,
            });
        }

        if let Some((_, bindings)) = self
            .bindings
            .iter_mut()
            .find(|(candidate, _)| *candidate == action)
        {
            *bindings = vec![binding];
        }
        Ok(())
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl ShortcutBinding {
    fn display(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.control_key() {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.shift_key() {
            parts.push("Shift".to_string());
        }
        if self.modifiers.alt_key() {
            parts.push("Alt".to_string());
        }
        if self.modifiers.super_key() {
            parts.push("Win".to_string());
        }
        parts.push(key_code_label(self.code));
        parts.join("+")
    }
}

fn key_code_label(code: KeyCode) -> String {
    match code {
        KeyCode::Space => "Space".to_string(),
        KeyCode::ArrowLeft => "←".to_string(),
        KeyCode::ArrowRight => "→".to_string(),
        KeyCode::ArrowUp => "↑".to_string(),
        KeyCode::ArrowDown => "↓".to_string(),
        KeyCode::Comma => ",".to_string(),
        KeyCode::Period => ".".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        other => {
            let name = format!("{other:?}");
            name.strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .unwrap_or(&name)
                .to_string()
        }
    }
}

fn is_modifier_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

fn dispatch_shortcut(app: &AppWindow, action: ShortcutAction) {
    app.invoke_reveal_controls();
    match action {
        ShortcutAction::PlayPause => app.invoke_play_pause(),
        ShortcutAction::Stop => app.invoke_stop(),
        ShortcutAction::Previous => app.invoke_previous_video(),
        ShortcutAction::Next => app.invoke_next_video(),
        ShortcutAction::SeekBackwardShort => app.invoke_seek_relative(-5.0),
        ShortcutAction::SeekForwardShort => app.invoke_seek_relative(5.0),
        ShortcutAction::SeekBackwardLong => app.invoke_seek_relative(-10.0),
        ShortcutAction::SeekForwardLong => app.invoke_seek_relative(10.0),
        ShortcutAction::FrameBackward => app.invoke_step_frame(-1),
        ShortcutAction::FrameForward => app.invoke_step_frame(1),
        ShortcutAction::VolumeDown => app.invoke_adjust_volume(-5.0),
        ShortcutAction::VolumeUp => app.invoke_adjust_volume(5.0),
        ShortcutAction::ToggleMute => app.invoke_toggle_mute(),
        ShortcutAction::ToggleLoop => app.invoke_toggle_loop(),
        ShortcutAction::ToggleFullscreen => app.invoke_toggle_fullscreen(),
        ShortcutAction::TogglePictureInPicture => app.invoke_toggle_picture_in_picture(),
        ShortcutAction::ToggleInfo => app.invoke_toggle_info(),
        ShortcutAction::TogglePlaylist => app.invoke_toggle_playlist(),
        ShortcutAction::ToggleShortcuts => app.invoke_toggle_shortcuts(),
        ShortcutAction::ToggleCommandPalette => app.invoke_toggle_command_palette(),
        ShortcutAction::OpenFile => app.invoke_open_file(),
        ShortcutAction::OpenFolder => app.invoke_open_folder(),
    }
}

struct PaletteCommandSpec {
    action: i32,
    title: &'static str,
    description: &'static str,
    shortcut: &'static str,
    keywords: &'static str,
    requires_media: bool,
}

const PALETTE_COMMANDS: &[PaletteCommandSpec] = &[
    PaletteCommandSpec {
        action: 0,
        title: "Открыть файл",
        description: "Выбрать видео или аудиофайл",
        shortcut: "Ctrl O",
        keywords: "open media video audio открыть медиа видео аудио",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 1,
        title: "Открыть папку",
        description: "Собрать плейлист из папки",
        shortcut: "Ctrl Shift O",
        keywords: "open folder directory playlist открыть каталог плейлист",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 2,
        title: "Пауза / воспроизведение",
        description: "Переключить состояние плеера",
        shortcut: "Space",
        keywords: "play pause start stop играть пауза воспроизведение",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 3,
        title: "Предыдущий файл",
        description: "Перейти назад по плейлисту",
        shortcut: "P",
        keywords: "previous back playlist предыдущий назад",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 4,
        title: "Следующий файл",
        description: "Перейти вперёд по плейлисту",
        shortcut: "N",
        keywords: "next forward playlist следующий вперед вперёд",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 13,
        title: "Назад на 10 секунд",
        description: "Перемотать текущий файл назад",
        shortcut: "J",
        keywords: "seek rewind back перемотка назад",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 14,
        title: "Вперёд на 10 секунд",
        description: "Перемотать текущий файл вперёд",
        shortcut: "L",
        keywords: "seek forward перемотка вперед вперёд",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 25,
        title: "Предыдущий кадр",
        description: "Поставить видео на паузу и перейти на кадр назад",
        shortcut: ",",
        keywords: "frame back previous кадр назад предыдущий покадрово",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 26,
        title: "Следующий кадр",
        description: "Поставить видео на паузу и перейти на кадр вперёд",
        shortcut: ".",
        keywords: "frame forward next кадр вперед вперёд следующий покадрово",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 5,
        title: "Выключить / включить звук",
        description: "Переключить mute",
        shortcut: "M",
        keywords: "mute unmute sound audio звук без звука",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 15,
        title: "Уменьшить громкость",
        description: "Убавить громкость на 5%",
        shortcut: "↓",
        keywords: "volume down sound громкость тише убавить",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 16,
        title: "Увеличить громкость",
        description: "Прибавить громкость на 5%",
        shortcut: "↑",
        keywords: "volume up sound громкость громче прибавить",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 6,
        title: "Повтор файла",
        description: "Включить или выключить повтор",
        shortcut: "R",
        keywords: "repeat loop повтор цикл",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 7,
        title: "Показать плейлист",
        description: "Открыть или закрыть очередь файлов",
        shortcut: "Ctrl L",
        keywords: "playlist queue список очередь файлы",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 8,
        title: "Свойства файла",
        description: "Показать информацию о медиа",
        shortcut: "I",
        keywords: "info properties metadata свойства информация метаданные",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 9,
        title: "Настройки воспроизведения",
        description: "Скорость, повтор папки и перемешивание",
        shortcut: "—",
        keywords: "settings speed shuffle настройки скорость перемешивание",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 10,
        title: "Горячие клавиши",
        description: "Показать справку по управлению",
        shortcut: "H",
        keywords: "help shortcuts keyboard помощь клавиши управление",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 11,
        title: "Полноэкранный режим",
        description: "Переключить fullscreen",
        shortcut: "F",
        keywords: "fullscreen full screen полный экран",
        requires_media: false,
    },
    PaletteCommandSpec {
        action: 27,
        title: "Картинка в картинке",
        description: "Переключить компактное окно поверх остальных",
        shortcut: "Alt P",
        keywords: "picture in picture pip mini player окно поверх компактное",
        requires_media: true,
    },
    PaletteCommandSpec {
        action: 12,
        title: "Остановить воспроизведение",
        description: "Закрыть текущий файл",
        shortcut: "S",
        keywords: "stop close остановить закрыть",
        requires_media: true,
    },
];

fn command_palette_items_with_registry(
    query: &str,
    shortcuts: &ShortcutRegistry,
) -> Vec<CommandEntry> {
    let words = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    PALETTE_COMMANDS
        .iter()
        .filter(|command| {
            let searchable = format!(
                "{} {} {}",
                command.title, command.description, command.keywords
            )
            .to_lowercase();
            words.iter().all(|word| searchable.contains(word))
        })
        .map(|command| CommandEntry {
            action: command.action,
            title: command.title.into(),
            description: command.description.into(),
            shortcut: ShortcutAction::from_palette_action(command.action)
                .map(|action| shortcuts.display_for(action))
                .unwrap_or_else(|| command.shortcut.to_string())
                .into(),
            requires_media: command.requires_media,
        })
        .collect()
}

#[cfg(test)]
fn command_palette_items(query: &str) -> Vec<CommandEntry> {
    command_palette_items_with_registry(query, &ShortcutRegistry::default())
}

fn set_command_palette_items(
    model: &VecModel<CommandEntry>,
    query: &str,
    shortcuts: &ShortcutRegistry,
) {
    model.set_vec(command_palette_items_with_registry(query, shortcuts));
}

fn set_shortcut_items(app: &AppWindow, shortcuts: &ShortcutRegistry) {
    let items = ShortcutAction::ALL
        .into_iter()
        .map(|action| ShortcutEntry {
            action: action as i32,
            title: action.title().into(),
            shortcut: shortcuts.display_for(action).into(),
        })
        .collect::<Vec<_>>();
    app.set_shortcut_items(ModelRc::new(VecModel::from(items)));
}

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

fn format_container(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "—".to_owned();
    }

    let lower = value.to_ascii_lowercase();
    if lower.contains("matroska") || lower.contains("webm") {
        "Matroska / WebM".to_owned()
    } else if lower.contains("mp4") || lower.contains("mov") {
        "MPEG-4 / MOV".to_owned()
    } else {
        value.to_uppercase()
    }
}

fn format_codec(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "—".to_owned()
    } else {
        value.to_uppercase()
    }
}

fn format_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    if bytes == 0 {
        "—".to_owned()
    } else if bytes as f64 >= GIB {
        format!("{:.2} ГБ", bytes as f64 / GIB).replace('.', ",")
    } else if bytes as f64 >= MIB {
        format!("{:.1} МБ", bytes as f64 / MIB).replace('.', ",")
    } else if bytes as f64 >= KIB {
        format!("{:.0} КБ", bytes as f64 / KIB)
    } else {
        format!("{bytes} Б")
    }
}

fn format_bitrate(bits_per_second: f64) -> String {
    if !bits_per_second.is_finite() || bits_per_second <= 0.0 {
        "—".to_owned()
    } else if bits_per_second >= 1_000_000.0 {
        format!("{:.2} Мбит/с", bits_per_second / 1_000_000.0).replace('.', ",")
    } else {
        format!("{:.0} кбит/с", bits_per_second / 1_000.0)
    }
}

fn format_track_summary(video: u32, audio: u32, subtitles: u32) -> String {
    format!("Видео {video}  ·  Аудио {audio}  ·  Субтитры {subtitles}")
}

fn is_audio_only_media(width: u32, height: u32, video_codec: &str, audio_codec: &str) -> bool {
    width == 0 && height == 0 && video_codec.is_empty() && !audio_codec.is_empty()
}

fn command_for_dropped_path(path: &Path) -> Result<Command, String> {
    if path.is_dir() {
        return Ok(Command::OpenFolder(path.to_string_lossy().into_owned()));
    }
    if path.is_file() && is_media_file(path) {
        return Ok(Command::OpenFile(path.to_string_lossy().into_owned()));
    }

    Err(format!(
        "Перетащенный объект не является поддерживаемым медиафайлом или папкой: {}",
        path.display()
    ))
}

#[cfg(target_os = "windows")]
fn sync_video_host(host: &mut VideoHost, app: &AppWindow) -> Result<()> {
    let size = app.window().size();
    sync_video_host_at_size(
        host,
        app,
        (size.width, size.height),
        app.window().scale_factor(),
    )
}

#[cfg(target_os = "windows")]
fn sync_video_host_at_size(
    host: &mut VideoHost,
    app: &AppWindow,
    physical_size: (u32, u32),
    scale: f32,
) -> Result<()> {
    let video_visible = app.get_media_presented()
        && !app.get_audio_media()
        && !app.get_modal_overlay_active()
        && !app.get_error_visible()
        && !app.get_drop_target_visible();
    host.set_visible(video_visible);

    host.resize(
        physical_size,
        scale,
        (
            app.get_video_left_panel_inset(),
            app.get_video_right_panel_inset(),
            app.get_video_top_inset(),
            app.get_video_bottom_inset(),
        ),
    )
}

#[cfg(target_os = "windows")]
fn native_window_handle(window: &slint::Window) -> Option<HWND> {
    let window_handle = window.window_handle();
    let handle = window_handle.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

#[cfg(target_os = "windows")]
fn request_native_redraw(target: usize) {
    unsafe {
        RedrawWindow(
            target as HWND,
            std::ptr::null(),
            std::ptr::null_mut(),
            RDW_INTERNALPAINT,
        );
    }
}

#[cfg(target_os = "windows")]
fn cursor_screen_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    Some((point.x, point.y))
}

#[cfg(target_os = "windows")]
fn restored_window_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        flags: 0,
        showCmd: 0,
        ptMinPosition: POINT { x: 0, y: 0 },
        ptMaxPosition: POINT { x: 0, y: 0 },
        rcNormalPosition: Default::default(),
    };
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return None;
    }
    let rect = placement.rcNormalPosition;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(target_os = "windows")]
fn begin_manual_window_drag(window: &slint::Window, state: &Arc<Mutex<WindowDragState>>) -> bool {
    let Some(cursor_origin) = cursor_screen_position() else {
        return false;
    };
    let Some(hwnd) = native_window_handle(window) else {
        return false;
    };
    let Some(window_origin) = window
        .with_winit_window(|window| {
            let current_origin = window.outer_position().ok()?;
            if !window.is_maximized() {
                return Some(current_origin);
            }

            let current_size = window.outer_size();
            let restored_size = restored_window_size(hwnd)
                .unwrap_or((current_size.width as i32, current_size.height as i32));
            let anchor_x = ((cursor_origin.0 - current_origin.x) as f64
                / f64::from(current_size.width.max(1)))
            .clamp(0.05, 0.95);
            let title_height = (48.0 * window.scale_factor()).round() as i32;
            let anchor_y = (cursor_origin.1 - current_origin.y).clamp(0, title_height);
            let restored_origin = PhysicalPosition::new(
                cursor_origin.0 - (f64::from(restored_size.0) * anchor_x).round() as i32,
                cursor_origin.1 - anchor_y,
            );

            window.set_maximized(false);
            window.set_outer_position(restored_origin);
            Some(restored_origin)
        })
        .flatten()
    else {
        return false;
    };

    if let Ok(mut state) = state.lock() {
        *state = WindowDragState {
            active: true,
            cursor_origin,
            window_origin: (window_origin.x, window_origin.y),
        };
        unsafe {
            SetCapture(hwnd);
        }
        true
    } else {
        false
    }
}

#[cfg(target_os = "windows")]
fn update_manual_window_drag(window: &slint::Window, state: &Arc<Mutex<WindowDragState>>) {
    let drag = state
        .lock()
        .ok()
        .and_then(|state| state.active.then_some(*state));
    let (Some(drag), Some(cursor)) = (drag, cursor_screen_position()) else {
        return;
    };

    window.with_winit_window(|window| {
        window.set_outer_position(PhysicalPosition::new(
            drag.window_origin.0 + cursor.0 - drag.cursor_origin.0,
            drag.window_origin.1 + cursor.1 - drag.cursor_origin.1,
        ));
    });
}

#[cfg(target_os = "windows")]
fn finish_manual_window_drag(state: &Arc<Mutex<WindowDragState>>) -> bool {
    let active = state
        .lock()
        .map(|mut state| std::mem::take(&mut state.active))
        .unwrap_or(false);
    if active {
        unsafe {
            ReleaseCapture();
        }
    }
    active
}

#[cfg(target_os = "windows")]
fn sync_native_window_resizable(window: &slint::Window, fullscreen: bool) {
    window.with_winit_window(|window| {
        window.set_resizable(!fullscreen);
    });
}

#[cfg(target_os = "windows")]
fn activate_hwnd_video_fallback(
    slot: &mut Option<VideoHost>,
    app: &AppWindow,
    tx: &Sender<Command>,
) -> Result<()> {
    let mut host = VideoHost::create(app.window())?;
    sync_video_host(&mut host, app)?;
    let _ = tx.send(Command::SetVideoWindow(host.handle()));
    *slot = Some(host);
    app.set_integrated_video_renderer(false);
    Ok(())
}

#[cfg(target_os = "windows")]
fn toggle_native_fullscreen(app: &AppWindow, native_fullscreen: &Arc<Mutex<NativeFullscreen>>) {
    let fullscreen = !app.get_fullscreen_mode();
    if fullscreen {
        app.set_fullscreen_mode(true);
    }

    let result = native_fullscreen
        .lock()
        .map_err(|_| anyhow::anyhow!("Не удалось изменить полноэкранный режим"))
        .and_then(|mut native_fullscreen| native_fullscreen.set(app.window(), fullscreen));

    if let Err(error) = result {
        if fullscreen {
            app.set_fullscreen_mode(false);
        }
        app.set_error_message(error.to_string().into());
        app.set_error_visible(true);
    } else if !fullscreen {
        app.set_fullscreen_mode(false);
    }
    sync_native_window_resizable(app.window(), app.get_fullscreen_mode());
}

#[cfg(target_os = "windows")]
fn toggle_picture_in_picture(
    app: &AppWindow,
    state: &Arc<Mutex<PictureInPictureState>>,
    native_fullscreen: &Arc<Mutex<NativeFullscreen>>,
) {
    let entering = !app.get_picture_in_picture();
    if entering && app.get_fullscreen_mode() {
        toggle_native_fullscreen(app, native_fullscreen);
        if app.get_fullscreen_mode() {
            return;
        }
    }

    if entering {
        app.set_info_drawer_visible(false);
        app.set_playlist_drawer_visible(false);
        app.set_settings_drawer_visible(false);
        app.set_inspector_reserved(false);
        app.set_shortcut_editor_visible(false);
        app.set_command_palette_visible(false);
        app.set_picture_in_picture(true);
    }

    let result = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Не удалось изменить режим «картинка в картинке»"))
        .map(|mut state| {
            app.window().with_winit_window(|window| {
                if entering {
                    if state.active {
                        return;
                    }
                    state.saved_position = window.outer_position().ok();
                    state.saved_size = Some(window.inner_size());
                    state.was_maximized = window.is_maximized();

                    window.set_minimized(false);
                    window.set_maximized(false);

                    let target_logical = LogicalSize::new(480.0, 300.0);
                    let target_physical: PhysicalSize<u32> =
                        target_logical.to_physical(window.scale_factor());
                    let _ = window.request_inner_size(target_physical);

                    if let Some(monitor) = window.current_monitor() {
                        let monitor_position = monitor.position();
                        let monitor_size = monitor.size();
                        let right_offset = monitor_size
                            .width
                            .saturating_sub(target_physical.width.saturating_add(32));
                        let bottom_offset = monitor_size
                            .height
                            .saturating_sub(target_physical.height.saturating_add(72));
                        window.set_outer_position(PhysicalPosition::new(
                            monitor_position.x + i32::try_from(right_offset).unwrap_or(i32::MAX),
                            monitor_position.y + i32::try_from(bottom_offset).unwrap_or(i32::MAX),
                        ));
                    }
                    state.active = true;
                } else if state.active {
                    window.set_maximized(false);
                    if let Some(size) = state.saved_size {
                        let _ = window.request_inner_size(size);
                    }
                    if let Some(position) = state.saved_position {
                        window.set_outer_position(position);
                    }
                    if state.was_maximized {
                        window.set_maximized(true);
                    }
                    *state = PictureInPictureState::default();
                }
                window.request_redraw();
            });
        });

    if let Err(error) = result {
        if entering {
            app.set_picture_in_picture(false);
        }
        app.set_error_message(error.to_string().into());
        app.set_error_visible(true);
    } else if !entering {
        app.set_picture_in_picture(false);
    }
    app.invoke_reveal_controls();
}

pub fn run(tx: Sender<Command>, rx: Receiver<Event>) -> Result<()> {
    let app = AppWindow::new()?;
    app.set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    let shortcut_registry = Arc::new(Mutex::new(ShortcutRegistry::default()));
    let command_items = Rc::new(VecModel::default());
    app.set_command_items(ModelRc::from(command_items.clone()));
    if let Ok(shortcuts) = shortcut_registry.lock() {
        set_command_palette_items(&command_items, "", &shortcuts);
        set_shortcut_items(&app, &shortcuts);
    }
    let current_duration = Arc::new(Mutex::new(0.0));
    let dialog_active = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let video_host = Arc::new(Mutex::new(None::<VideoHost>));
    #[cfg(target_os = "windows")]
    let render_source = Arc::new(Mutex::new(None::<MpvRenderSource>));
    #[cfg(target_os = "windows")]
    let native_fullscreen = Arc::new(Mutex::new(NativeFullscreen::default()));
    #[cfg(target_os = "windows")]
    let picture_in_picture_state = Arc::new(Mutex::new(PictureInPictureState::default()));
    #[cfg(target_os = "windows")]
    let video_resize_timer = slint::Timer::default();
    #[cfg(target_os = "windows")]
    let window_drag_state = Arc::new(Mutex::new(WindowDragState::default()));

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let window_drag_state = window_drag_state.clone();
        app.on_drag_window(move || {
            if let Some(app) = app_weak.upgrade() {
                let window = app.window();
                if app.get_fullscreen_mode() {
                    return;
                }
                if !begin_manual_window_drag(window, &window_drag_state) {
                    window.with_winit_window(|window| {
                        let _ = window.drag_window();
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
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
        let shortcut_registry = shortcut_registry.clone();
        let event_command_items = command_items.clone();
        #[cfg(target_os = "windows")]
        let event_window_drag_state = window_drag_state.clone();
        let mut modifiers = ModifiersState::empty();
        let mut last_cursor_position = None::<(f64, f64)>;
        let mut drawer_swipe = None::<(DrawerSwipeTarget, f64)>;
        let mut theme_initialized = false;

        app.window().on_winit_window_event(move |window, event| {
            let Some(app) = app_weak.upgrade() else {
                return EventResult::Propagate;
            };

            if !theme_initialized {
                let dark_theme = window
                    .with_winit_window(|native_window| {
                        native_window
                            .theme()
                            .map_or(app.get_dark_theme(), |theme| theme == WinitTheme::Dark)
                    })
                    .unwrap_or_else(|| app.get_dark_theme());
                let _ = tx.send(Command::SetDarkTheme(dark_theme));
                theme_initialized = true;
            }

            match event {
                WindowEvent::CursorMoved { position, .. } => {
                    #[cfg(target_os = "windows")]
                    update_manual_window_drag(window, &event_window_drag_state);

                    let scale = f64::from(app.window().scale_factor());
                    let window_height = f64::from(app.window().size().height);
                    let top_zone = f64::from(app.get_controls_top_zone()) * scale;
                    let bottom_zone = f64::from(app.get_controls_bottom_zone()) * scale;
                    app.set_controls_pointer_over(
                        position.y <= top_zone || position.y >= window_height - bottom_zone,
                    );
                    let current_position = (position.x, position.y);
                    if last_cursor_position != Some(current_position) {
                        last_cursor_position = Some(current_position);
                        app.invoke_reveal_controls();
                    }
                    if let Some((target, start_x)) = drawer_swipe {
                        let delta = position.x - start_x;
                        let threshold = 48.0 * scale;
                        let should_close = delta > threshold;
                        if should_close {
                            drawer_swipe = None;
                            match target {
                                DrawerSwipeTarget::Info => app.invoke_toggle_info(),
                                DrawerSwipeTarget::Playlist => app.invoke_toggle_playlist(),
                                DrawerSwipeTarget::Settings => app.invoke_toggle_settings(),
                            }
                        }
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    app.set_controls_pointer_over(false);
                }
                WindowEvent::Focused(focused) => {
                    app.set_window_active(*focused);
                    #[cfg(target_os = "windows")]
                    if !focused {
                        finish_manual_window_drag(&event_window_drag_state);
                    }
                }
                WindowEvent::ThemeChanged(theme) => {
                    let _ = tx.send(Command::SetDarkTheme(matches!(theme, WinitTheme::Dark)));
                    window.request_redraw();
                }
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    // Keep Slint and the native mpv child on the same frame. Resizing the child
                    // here makes video geometry lead the FemtoVG control deck by one paint.
                    // The AfterRendering notifier commits both dimensions together.
                    #[cfg(target_os = "windows")]
                    sync_native_window_resizable(window, app.get_fullscreen_mode());
                    window.request_redraw();
                }
                WindowEvent::Moved(_) => {
                    window.request_redraw();
                }
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    if state.is_pressed() {
                        let Some((pointer_x, pointer_y)) = last_cursor_position else {
                            return EventResult::Propagate;
                        };
                        let scale = f64::from(app.window().scale_factor());
                        let window_size = app.window().size();
                        let window_width = f64::from(window_size.width);
                        let window_height = f64::from(window_size.height);
                        let logical_width = window_width / scale;
                        let panel_top = f64::from(app.get_controls_top_zone()) * scale;
                        let panel_bottom = window_height
                            - (f64::from(app.get_controls_bottom_zone()) + 4.0) * scale;

                        if pointer_y >= panel_top && pointer_y <= panel_bottom {
                            let inspector_width = logical_width.min(392.0 + 32.0) - 32.0;
                            let inspector_start = (logical_width - inspector_width - 16.0) * scale;
                            let inspector_end = (logical_width - 16.0) * scale;

                            let target =
                                if pointer_x >= inspector_start && pointer_x <= inspector_end {
                                    if app.get_playlist_drawer_visible() {
                                        Some(DrawerSwipeTarget::Playlist)
                                    } else if app.get_info_drawer_visible() {
                                        Some(DrawerSwipeTarget::Info)
                                    } else if app.get_settings_drawer_visible() {
                                        Some(DrawerSwipeTarget::Settings)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                            drawer_swipe = target.map(|target| (target, pointer_x));
                        }
                    } else {
                        drawer_swipe = None;
                        #[cfg(target_os = "windows")]
                        finish_manual_window_drag(&event_window_drag_state);
                    }
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                    let PhysicalKey::Code(code) = event.physical_key else {
                        return EventResult::Propagate;
                    };

                    #[cfg(target_os = "windows")]
                    if code == KeyCode::Escape
                        && finish_manual_window_drag(&event_window_drag_state)
                    {
                        return EventResult::PreventDefault;
                    }

                    if app.get_resume_prompt_visible() {
                        if code == KeyCode::Enter {
                            app.invoke_resume_saved_playback();
                        } else if code == KeyCode::Escape {
                            app.invoke_restart_saved_playback();
                        }
                        return EventResult::PreventDefault;
                    }

                    if app.get_shortcut_editor_visible() {
                        if code == KeyCode::Escape {
                            if app.get_shortcut_capture_action() >= 0 {
                                app.set_shortcut_capture_action(-1);
                                app.set_shortcut_editor_error("".into());
                            } else {
                                app.set_shortcut_editor_visible(false);
                            }
                            return EventResult::PreventDefault;
                        }

                        let capture_action = app.get_shortcut_capture_action();
                        if capture_action >= 0 {
                            if event.repeat || is_modifier_key(code) {
                                return EventResult::PreventDefault;
                            }
                            let Some(action) = ShortcutAction::from_id(capture_action) else {
                                app.set_shortcut_capture_action(-1);
                                return EventResult::PreventDefault;
                            };
                            let binding = ShortcutBinding::new(code, modifiers);
                            if let Ok(mut shortcuts) = shortcut_registry.lock() {
                                match shortcuts.rebind(action, binding) {
                                    Ok(()) => {
                                        set_shortcut_items(&app, &shortcuts);
                                        set_command_palette_items(
                                            &event_command_items,
                                            app.get_command_query().as_str(),
                                            &shortcuts,
                                        );
                                        app.set_shortcut_capture_action(-1);
                                        app.set_shortcut_editor_error("".into());
                                    }
                                    Err(conflict) => {
                                        app.set_shortcut_editor_error(
                                            format!(
                                                "{} уже используется для действия «{}»",
                                                conflict.binding.display(),
                                                conflict.action.title()
                                            )
                                            .into(),
                                        );
                                    }
                                }
                            }
                            return EventResult::PreventDefault;
                        }

                        let binding = ShortcutBinding::new(code, modifiers);
                        let action = shortcut_registry
                            .lock()
                            .ok()
                            .and_then(|shortcuts| shortcuts.action_for(binding));
                        if action == Some(ShortcutAction::ToggleShortcuts) {
                            dispatch_shortcut(&app, ShortcutAction::ToggleShortcuts);
                            return EventResult::PreventDefault;
                        }
                        if action.is_some() {
                            return EventResult::PreventDefault;
                        }
                        return EventResult::Propagate;
                    }

                    if code == KeyCode::Escape && app.get_inspector_visible() {
                        if app.get_playlist_drawer_visible() {
                            app.invoke_toggle_playlist();
                        } else if app.get_info_drawer_visible() {
                            app.invoke_toggle_info();
                        } else if app.get_settings_drawer_visible() {
                            app.invoke_toggle_settings();
                        }
                        return EventResult::PreventDefault;
                    }

                    let binding = ShortcutBinding::new(code, modifiers);
                    let Some(action) = shortcut_registry
                        .lock()
                        .ok()
                        .and_then(|shortcuts| shortcuts.action_for(binding))
                    else {
                        return EventResult::Propagate;
                    };

                    if app.get_command_palette_visible()
                        && action != ShortcutAction::ToggleCommandPalette
                    {
                        return EventResult::Propagate;
                    }
                    if !event.repeat || action.allows_repeat() {
                        dispatch_shortcut(&app, action);
                    }
                    return EventResult::PreventDefault;
                }
                WindowEvent::HoveredFile(path) => {
                    app.set_drop_target_visible(path.is_dir() || is_media_file(path));
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
                    .add_filter("Медиа", MEDIA_EXTENSIONS)
                    .add_filter("Видео", VIDEO_EXTENSIONS)
                    .add_filter("Аудио", AUDIO_EXTENSIONS)
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
        app.on_open_recent(move |path| {
            let _ = tx.send(Command::OpenFile(path.to_string()));
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
        let dialog_active = dialog_active.clone();

        app.on_load_subtitle(move || {
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
                    .add_filter("Субтитры", SUBTITLE_EXTENSIONS)
                    .pick_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::LoadSubtitle(path.to_string_lossy().into_owned()));
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

    {
        let tx = tx.clone();
        app.on_set_hardware_decoding(move |enabled| {
            let _ = tx.send(Command::SetHardwareDecoding(enabled));
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

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let native_fullscreen = native_fullscreen.clone();
        let picture_in_picture_state = picture_in_picture_state.clone();

        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                if app.get_picture_in_picture() {
                    toggle_picture_in_picture(&app, &picture_in_picture_state, &native_fullscreen);
                }
                toggle_native_fullscreen(&app, &native_fullscreen);
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let native_fullscreen = native_fullscreen.clone();
        let picture_in_picture_state = picture_in_picture_state.clone();

        app.on_toggle_picture_in_picture(move || {
            if let Some(app) = app_weak.upgrade()
                && app.get_media_loaded()
            {
                toggle_picture_in_picture(&app, &picture_in_picture_state, &native_fullscreen);
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();

        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                let fullscreen = !app.window().is_fullscreen();
                app.set_fullscreen_mode(fullscreen);
                app.window().set_fullscreen(fullscreen);
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();

        app.on_toggle_picture_in_picture(move || {
            if let Some(app) = app_weak.upgrade()
                && app.get_media_loaded()
            {
                app.set_picture_in_picture(!app.get_picture_in_picture());
                app.invoke_reveal_controls();
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
                    app.set_inspector_reserved(true);
                    app.set_info_drawer_visible(true);
                    app.set_playlist_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                    if let Ok(mut video_host) = panel_video_host.lock()
                        && let Some(host) = video_host.as_mut()
                    {
                        let _ = sync_video_host(host, &app);
                    }
                } else {
                    app.set_info_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(DRAWER_CLOSE_DELAY, move || {
                        if let Some(app) = delayed_app.upgrade()
                            && !app.get_inspector_visible()
                        {
                            app.set_inspector_reserved(false);
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
                    app.set_info_drawer_visible(true);
                    app.set_playlist_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                } else {
                    app.set_info_drawer_visible(false);
                }
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
                    app.set_inspector_reserved(true);
                    app.set_playlist_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                    if let Ok(mut video_host) = panel_video_host.lock()
                        && let Some(host) = video_host.as_mut()
                    {
                        let _ = sync_video_host(host, &app);
                    }
                } else {
                    app.set_playlist_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(DRAWER_CLOSE_DELAY, move || {
                        if let Some(app) = delayed_app.upgrade()
                            && !app.get_inspector_visible()
                        {
                            app.set_inspector_reserved(false);
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
                    app.set_playlist_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                } else {
                    app.set_playlist_drawer_visible(false);
                }
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
                    app.set_inspector_reserved(true);
                    app.set_settings_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_playlist_drawer_visible(false);
                    if let Ok(mut video_host) = panel_video_host.lock()
                        && let Some(host) = video_host.as_mut()
                    {
                        let _ = sync_video_host(host, &app);
                    }
                } else {
                    app.set_settings_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(DRAWER_CLOSE_DELAY, move || {
                        if let Some(app) = delayed_app.upgrade()
                            && !app.get_inspector_visible()
                        {
                            app.set_inspector_reserved(false);
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
                let opening = !app.get_settings_drawer_visible();
                if opening {
                    app.set_settings_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_playlist_drawer_visible(false);
                } else {
                    app.set_settings_drawer_visible(false);
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_toggle_shortcuts(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_shortcut_editor_visible();
                app.set_command_palette_visible(false);
                app.set_shortcut_capture_action(-1);
                app.set_shortcut_editor_error("".into());
                app.set_shortcut_editor_visible(opening);
                if opening {
                    app.set_info_drawer_visible(false);
                    app.set_playlist_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                    app.set_inspector_reserved(false);
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_toggle_command_palette(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_command_palette_visible();
                app.set_shortcut_editor_visible(false);
                app.set_shortcut_capture_action(-1);
                app.set_shortcut_editor_error("".into());
                app.set_command_query("".into());
                if !opening {
                    app.invoke_command_query_changed("".into());
                }
                app.set_command_palette_visible(opening);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let command_items = command_items.clone();

        app.on_command_query_changed(move |query| {
            if app_weak.upgrade().is_some()
                && let Ok(shortcuts) = shortcut_registry.lock()
            {
                set_command_palette_items(&command_items, query.as_str(), &shortcuts);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let command_items = command_items.clone();

        app.on_reset_shortcuts(move || {
            if let Some(app) = app_weak.upgrade() {
                if let Ok(mut shortcuts) = shortcut_registry.lock() {
                    shortcuts.reset();
                    set_shortcut_items(&app, &shortcuts);
                    set_command_palette_items(
                        &command_items,
                        app.get_command_query().as_str(),
                        &shortcuts,
                    );
                }
                app.set_shortcut_capture_action(-1);
                app.set_shortcut_editor_error("".into());
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

        app.on_step_frame(move |direction| {
            let direction = if direction < 0 {
                FrameStepDirection::Backward
            } else {
                FrameStepDirection::Forward
            };
            let _ = tx.send(Command::StepFrame(direction));
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
        let notifier_render_source = render_source.clone();
        let mut gl_renderer = None::<MpvOpenGlRenderer>;
        let mut render_api_failed = false;

        app.window()
            .set_rendering_notifier(move |state, graphics_api| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let Ok(mut video_host) = notifier_video_host.lock() else {
                    return;
                };

                match state {
                    slint::RenderingState::BeforeRendering => {
                        if gl_renderer.is_none() && video_host.is_none() && !render_api_failed {
                            let source = notifier_render_source
                                .lock()
                                .ok()
                                .and_then(|source| source.clone());

                            if let Some(source) = source {
                                let renderer = match graphics_api {
                                    slint::GraphicsAPI::NativeOpenGL { get_proc_address } => {
                                        let wake_app = app.as_weak();
                                        let native_redraw_hwnd =
                                            native_window_handle(app.window()).map(|hwnd| hwnd as usize);
                                        let redraw_pending = Arc::new(AtomicBool::new(false));
                                        let wake_pending = Arc::clone(&redraw_pending);
                                        MpvOpenGlRenderer::create(
                                            source,
                                            *get_proc_address,
                                            move || {
                                                if let Some(target) = native_redraw_hwnd {
                                                    request_native_redraw(target);
                                                }
                                                if wake_pending.swap(true, Ordering::AcqRel) {
                                                    return;
                                                }
                                                let event_pending = Arc::clone(&wake_pending);
                                                if wake_app
                                                    .upgrade_in_event_loop(move |app| {
                                                        event_pending.store(false, Ordering::Release);
                                                        app.window().request_redraw();
                                                    })
                                                    .is_err()
                                                {
                                                    wake_pending.store(false, Ordering::Release);
                                                }
                                            },
                                        )
                                    }
                                    _ => Err(anyhow::anyhow!(
                                        "FemtoVG did not expose a native OpenGL context"
                                    )),
                                };

                                match renderer {
                                    Ok(renderer) => {
                                        gl_renderer = Some(renderer);
                                        app.set_integrated_video_renderer(true);
                                        let _ = tx_handle.send(Command::RenderApiReady);
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "OpenGL video renderer unavailable, using HWND fallback: {error:#}"
                                        );
                                        render_api_failed = true;
                                        if let Err(error) = activate_hwnd_video_fallback(
                                            &mut video_host,
                                            &app,
                                            &tx_handle,
                                        ) {
                                            app.set_error_message(error.to_string().into());
                                            app.set_error_visible(true);
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(renderer) = gl_renderer.as_mut() {
                            let size = app.window().size();
                            if let Err(error) = renderer.render((size.width, size.height)) {
                                eprintln!("OpenGL video render failed: {error:#}");
                                gl_renderer = None;
                                render_api_failed = true;
                                app.set_integrated_video_renderer(false);
                                app.set_error_message(
                                    "Не удалось продолжить встроенный вывод видео".into(),
                                );
                                app.set_error_visible(true);
                            }
                        }
                    }
                    slint::RenderingState::AfterRendering => {
                        if let Some(renderer) = gl_renderer.as_ref() {
                            renderer.report_swap();
                        } else if let Some(host) = video_host.as_mut()
                            && let Err(error) = sync_video_host(host, &app)
                        {
                            app.set_error_message(error.to_string().into());
                            app.set_error_visible(true);
                        }
                    }
                    slint::RenderingState::RenderingTeardown => {
                        gl_renderer = None;
                        *video_host = None;
                    }
                    _ => {}
                }
            })
            .ok();

        let timer_app = app.as_weak();
        let timer_native_fullscreen = native_fullscreen.clone();
        video_resize_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                let Some(app) = timer_app.upgrade() else {
                    return;
                };
                if VideoHost::take_fullscreen_request() {
                    toggle_native_fullscreen(&app, &timer_native_fullscreen);
                }
                if VideoHost::take_pointer_activity() {
                    app.invoke_reveal_controls();
                }
                if VideoHost::take_primary_click() {
                    if app.get_shortcut_editor_visible() {
                        app.invoke_toggle_shortcuts();
                    }
                    if app.get_info_drawer_visible() {
                        app.invoke_toggle_info();
                    }
                    if app.get_playlist_drawer_visible() {
                        app.invoke_toggle_playlist();
                    }
                    if app.get_settings_drawer_visible() {
                        app.invoke_toggle_settings();
                    }
                }
            },
        );
    }

    let weak = app.as_weak();
    #[cfg(target_os = "windows")]
    let event_render_source = render_source.clone();

    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            let weak = weak.clone();
            let current_duration = current_duration.clone();
            #[cfg(target_os = "windows")]
            let render_source = event_render_source.clone();

            slint::invoke_from_event_loop(move || {
                let Some(app) = weak.upgrade() else {
                    return;
                };

                match event {
                    #[cfg(target_os = "windows")]
                    Event::RenderSource(source) => {
                        if let Ok(mut target) = render_source.lock() {
                            *target = Some(source);
                        }
                        app.window().request_redraw();
                    }

                    #[cfg(not(target_os = "windows"))]
                    Event::RenderSource(_) => {}

                    Event::LoadStarted(name) => {
                        app.set_resume_prompt_visible(false);
                        let preserve_media_surface = app.get_media_presented();
                        app.set_file_name(name.into());
                        app.set_track_title("".into());
                        app.set_track_artist("".into());
                        app.set_album_title("".into());
                        app.set_album_art(Image::default());
                        app.set_album_art_available(false);
                        app.set_audio_tracks(ModelRc::new(VecModel::from(
                            Vec::<AudioTrackEntry>::new(),
                        )));
                        app.set_subtitle_tracks(ModelRc::new(VecModel::from(Vec::<
                            SubtitleTrackEntry,
                        >::new(
                        ))));
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

                    Event::FileLoaded(name) => {
                        app.set_file_name(name.into());
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
                        if let Ok(mut duration) = current_duration.lock() {
                            *duration = 0.0;
                        }
                    }

                    Event::MediaAvailable(available) => {
                        app.set_media_loaded(available);
                        app.set_media_transitioning(
                            available && app.get_waiting_for_video_surface(),
                        );
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

                    Event::Volume(v) => {
                        app.set_volume((v / 100.0) as f32);
                    }

                    Event::Muted(muted) => {
                        app.set_muted(muted);
                    }

                    Event::Speed(v) => {
                        app.set_speed(v as f32);
                    }

                    Event::AudioDelay(value) => {
                        app.set_audio_delay(value as f32);
                    }

                    Event::SubtitleDelay(value) => {
                        app.set_subtitle_delay(value as f32);
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

                    Event::RecentFiles(files) => {
                        let files = files
                            .into_iter()
                            .map(|file| RecentFileEntry {
                                name: file.name.into(),
                                path: file.path.into(),
                                location: file.location.into(),
                            })
                            .collect::<Vec<_>>();
                        app.set_recent_files(ModelRc::new(VecModel::from(files)));
                    }

                    Event::ResumeAvailable(position) => {
                        app.set_resume_position_label(format_time(position).into());
                        app.set_resume_prompt_visible(true);
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
                    } => {
                        let audio_media = is_audio_only_media(width, height, &codec, &audio_codec);
                        let wait_for_video =
                            app.get_media_transitioning() && app.get_audio_media() && !audio_media;
                        app.set_waiting_for_video_surface(wait_for_video);
                        if !wait_for_video {
                            app.set_audio_media(audio_media);
                        }
                        app.set_resolution(if width > 0 && height > 0 {
                            format!("{width}×{height}").into()
                        } else {
                            "—".into()
                        });
                        app.set_fps(if fps.is_finite() && fps > 0.0 {
                            format!("{fps:.2} FPS").into()
                        } else {
                            "—".into()
                        });
                        app.set_codec(format_codec(&codec).into());
                        app.set_audio_codec(format_codec(&audio_codec).into());
                        app.set_container_format(format_container(&container).into());
                        app.set_file_size_label(format_file_size(file_size_bytes).into());
                        app.set_media_duration_label(
                            if duration_seconds.is_finite() && duration_seconds > 0.0 {
                                format_time(duration_seconds).into()
                            } else {
                                "—".into()
                            },
                        );
                        app.set_media_bitrate_label(format_bitrate(average_bitrate_bps).into());
                        app.set_track_summary(
                            format_track_summary(
                                video_track_count,
                                audio_track_count,
                                subtitle_track_count,
                            )
                            .into(),
                        );
                    }

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

    use super::{
        PALETTE_COMMANDS, ShortcutAction, ShortcutBinding, ShortcutRegistry,
        command_for_dropped_path, command_palette_items, format_bitrate, format_container,
        format_file_size, format_time, format_track_summary, is_audio_only_media,
    };
    use crate::ipc::Command;
    use slint::winit_030::winit::keyboard::{KeyCode, ModifiersState};

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
    fn formats_local_media_properties_for_the_inspector() {
        assert_eq!(format_container("mov,mp4,m4a,3gp"), "MPEG-4 / MOV");
        assert_eq!(format_container("matroska,webm"), "Matroska / WebM");
        assert_eq!(format_file_size(1_048_576), "1,0 МБ");
        assert_eq!(format_file_size(1_073_741_824), "1,00 ГБ");
        assert_eq!(format_bitrate(800_000.0), "800 кбит/с");
        assert_eq!(format_bitrate(2_500_000.0), "2,50 Мбит/с");
        assert_eq!(
            format_track_summary(1, 2, 3),
            "Видео 1  ·  Аудио 2  ·  Субтитры 3"
        );
    }

    #[test]
    fn detects_audio_only_media_from_actual_tracks_not_file_extension() {
        assert!(is_audio_only_media(0, 0, "", "MP3 (MPEG audio layer 3)"));
        assert!(!is_audio_only_media(1920, 1080, "yuv420p", "AAC"));
        assert!(!is_audio_only_media(0, 0, "", ""));
    }

    #[test]
    fn rejects_unsupported_dropped_paths() {
        assert!(command_for_dropped_path(Path::new("notes.txt")).is_err());
        assert!(command_for_dropped_path(Path::new("missing.mp4")).is_err());
    }

    #[test]
    fn command_palette_returns_all_commands_for_an_empty_query() {
        assert_eq!(command_palette_items("").len(), PALETTE_COMMANDS.len());
        assert_eq!(command_palette_items("   ").len(), PALETTE_COMMANDS.len());
    }

    #[test]
    fn command_palette_searches_titles_descriptions_and_aliases() {
        let mute = command_palette_items("mute");
        assert_eq!(mute.len(), 1);
        assert_eq!(mute[0].action, 5);

        let folder = command_palette_items("папку плейлист");
        assert_eq!(folder.len(), 1);
        assert_eq!(folder[0].action, 1);

        assert!(command_palette_items("несуществующая-команда").is_empty());
    }

    #[test]
    fn command_palette_marks_media_dependent_actions() {
        let open_file = command_palette_items("открыть файл");
        assert!(!open_file[0].requires_media);

        let seek = command_palette_items("перемотать назад");
        assert!(seek[0].requires_media);
    }

    #[test]
    fn shortcuts_are_resolved_from_physical_keys_and_exact_modifiers() {
        let shortcuts = ShortcutRegistry::default();
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::empty())),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::CONTROL)),
            Some(ShortcutAction::ToggleCommandPalette)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(
                KeyCode::KeyO,
                ModifiersState::CONTROL | ModifiersState::SHIFT
            )),
            Some(ShortcutAction::OpenFolder)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::ALT)),
            None
        );
    }

    #[test]
    fn rebind_replaces_aliases_for_an_action() {
        let mut shortcuts = ShortcutRegistry::default();
        let replacement = ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL);

        shortcuts
            .rebind(ShortcutAction::ToggleMute, replacement)
            .expect("unused shortcut must be accepted");

        assert_eq!(
            shortcuts.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            None
        );
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "Ctrl+B");
    }

    #[test]
    fn rebind_rejects_conflicts_without_changing_existing_bindings() {
        let mut shortcuts = ShortcutRegistry::default();
        let play_pause = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());

        let conflict = shortcuts
            .rebind(ShortcutAction::ToggleMute, play_pause)
            .expect_err("a shortcut assigned to another action must be rejected");

        assert_eq!(conflict.action, ShortcutAction::PlayPause);
        assert_eq!(conflict.binding, play_pause);
        assert_eq!(
            shortcuts.action_for(play_pause),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn reset_restores_default_shortcuts() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .expect("unused shortcut must be accepted");

        shortcuts.reset();

        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn only_continuous_shortcuts_repeat_while_held() {
        assert!(ShortcutAction::SeekForwardLong.allows_repeat());
        assert!(ShortcutAction::VolumeUp.allows_repeat());
        assert!(!ShortcutAction::ToggleMute.allows_repeat());
        assert!(!ShortcutAction::ToggleCommandPalette.allows_repeat());
    }

    #[test]
    fn creates_open_file_command_for_supported_media_drop() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        let video_path = std::env::temp_dir().join(format!("oxide-drop-{unique}.mkv"));
        let audio_path = std::env::temp_dir().join(format!("oxide-drop-{unique}.flac"));
        std::fs::write(&video_path, []).expect("temporary video must be created");
        std::fs::write(&audio_path, []).expect("temporary audio must be created");

        assert!(matches!(
            command_for_dropped_path(&video_path),
            Ok(Command::OpenFile(_))
        ));
        assert!(matches!(
            command_for_dropped_path(&audio_path),
            Ok(Command::OpenFile(_))
        ));

        std::fs::remove_file(video_path).expect("temporary video must be removed");
        std::fs::remove_file(audio_path).expect("temporary audio must be removed");
    }
}
