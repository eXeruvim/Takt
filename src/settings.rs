use crate::storage::Persistence;
#[cfg(test)]
use std::path::Path;
use std::{
    io::{Error, ErrorKind},
    path::PathBuf,
};

const SETTINGS_FORMAT: &str = "OXIDE_SETTINGS";
const SETTINGS_VERSION: u32 = 1;
const DEFAULT_VOLUME: f32 = 0.65;
const DEFAULT_SPEED: f32 = 1.0;
const MAX_SHORTCUT_OVERRIDES: usize = 64;
const MAX_SHORTCUT_KEY_BYTES: usize = 256;
const MIN_WINDOW_WIDTH: u32 = 760;
const MIN_WINDOW_HEIGHT: u32 = 520;
const MAX_WINDOW_DIMENSION: u32 = 32_768;
const MAX_WINDOW_COORDINATE: i32 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AccentPreference {
    #[default]
    Copper,
    Iris,
    Sage,
    Blue,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompletionBehavior {
    #[default]
    Stop,
    RepeatPlaylist,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutPreference {
    pub action_id: i32,
    /// Serialized physical key, or JSON `null` for an explicitly unassigned command.
    pub key_code: String,
    pub modifiers: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowPlacementPreference {
    pub x: i32,
    pub y: i32,
    /// Normal-window client width in logical pixels.
    pub width: u32,
    /// Normal-window client height in logical pixels.
    pub height: u32,
}

impl WindowPlacementPreference {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Option<Self> {
        (x.unsigned_abs() <= MAX_WINDOW_COORDINATE as u32
            && y.unsigned_abs() <= MAX_WINDOW_COORDINATE as u32
            && (MIN_WINDOW_WIDTH..=MAX_WINDOW_DIMENSION).contains(&width)
            && (MIN_WINDOW_HEIGHT..=MAX_WINDOW_DIMENSION).contains(&height))
        .then_some(Self {
            x,
            y,
            width,
            height,
        })
    }
}

impl CompletionBehavior {
    pub const fn repeats_playlist(self) -> bool {
        matches!(self, Self::RepeatPlaylist)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::RepeatPlaylist => "repeat-playlist",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "stop" => Some(Self::Stop),
            "repeat-playlist" => Some(Self::RepeatPlaylist),
            _ => None,
        }
    }
}

impl AccentPreference {
    pub fn from_ui_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Copper),
            1 => Some(Self::Iris),
            2 => Some(Self::Sage),
            3 => Some(Self::Blue),
            _ => None,
        }
    }

    pub const fn ui_index(self) -> i32 {
        match self {
            Self::Copper => 0,
            Self::Iris => 1,
            Self::Sage => 2,
            Self::Blue => 3,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Copper => "copper",
            Self::Iris => "iris",
            Self::Sage => "sage",
            Self::Blue => "blue",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "copper" => Some(Self::Copper),
            "iris" | "amber" => Some(Self::Iris),
            "sage" => Some(Self::Sage),
            "blue" => Some(Self::Blue),
            _ => None,
        }
    }
}

impl ThemePreference {
    pub fn from_ui_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::System),
            1 => Some(Self::Dark),
            2 => Some(Self::Light),
            _ => None,
        }
    }

    pub const fn ui_index(self) -> i32 {
        match self {
            Self::System => 0,
            Self::Dark => 1,
            Self::Light => 2,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "system" => Some(Self::System),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct AppSettings {
    theme: ThemePreference,
    accent: AccentPreference,
    default_volume: f32,
    default_speed: f32,
    completion_behavior: CompletionBehavior,
    hardware_decoding_enabled: bool,
    window_placement: Option<WindowPlacementPreference>,
    shortcut_overrides: Vec<ShortcutPreference>,
    persistence: Persistence,
    dirty: bool,
    forget_previous_pending: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            accent: AccentPreference::Copper,
            default_volume: DEFAULT_VOLUME,
            default_speed: DEFAULT_SPEED,
            completion_behavior: CompletionBehavior::Stop,
            hardware_decoding_enabled: false,
            window_placement: None,
            shortcut_overrides: Vec::new(),
            persistence: Persistence::default(),
            dirty: false,
            forget_previous_pending: false,
        }
    }
}

impl AppSettings {
    pub fn load_default() -> Self {
        let (persistence, mut settings) = Persistence::load(default_storage_path(), Self::parse);
        settings.dirty = persistence.needs_repair();
        settings.persistence = persistence;
        settings
    }

    pub fn take_storage_notice(&mut self) -> Option<String> {
        self.persistence.take_notice()
    }

    pub const fn theme(&self) -> ThemePreference {
        self.theme
    }

    pub const fn accent(&self) -> AccentPreference {
        self.accent
    }

    pub const fn default_volume(&self) -> f32 {
        self.default_volume
    }

    pub const fn default_speed(&self) -> f32 {
        self.default_speed
    }

    pub const fn completion_behavior(&self) -> CompletionBehavior {
        self.completion_behavior
    }

    pub const fn hardware_decoding_enabled(&self) -> bool {
        self.hardware_decoding_enabled
    }

    pub const fn window_placement(&self) -> Option<WindowPlacementPreference> {
        self.window_placement
    }

    pub fn shortcut_overrides(&self) -> &[ShortcutPreference] {
        &self.shortcut_overrides
    }

    pub fn set_theme(&mut self, theme: ThemePreference) {
        if self.theme != theme {
            self.theme = theme;
            self.dirty = true;
        }
    }

    pub fn set_accent(&mut self, accent: AccentPreference) {
        if self.accent != accent {
            self.accent = accent;
            self.dirty = true;
        }
    }

    pub fn set_default_volume(&mut self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            DEFAULT_VOLUME
        };
        if (self.default_volume - volume).abs() > f32::EPSILON {
            self.default_volume = volume;
            self.dirty = true;
        }
    }

    pub fn set_default_speed(&mut self, speed: f32) {
        let speed = if speed.is_finite() {
            speed.clamp(0.25, 4.0)
        } else {
            DEFAULT_SPEED
        };
        if (self.default_speed - speed).abs() > f32::EPSILON {
            self.default_speed = speed;
            self.dirty = true;
        }
    }

    pub fn set_completion_behavior(&mut self, behavior: CompletionBehavior) {
        if self.completion_behavior != behavior {
            self.completion_behavior = behavior;
            self.dirty = true;
        }
    }

    pub fn set_hardware_decoding_enabled(&mut self, enabled: bool) {
        if self.hardware_decoding_enabled != enabled {
            self.hardware_decoding_enabled = enabled;
            self.dirty = true;
        }
    }

    pub fn set_window_placement(&mut self, placement: WindowPlacementPreference) {
        if self.window_placement != Some(placement) {
            self.window_placement = Some(placement);
            self.dirty = true;
        }
    }

    pub fn set_shortcut_overrides(&mut self, overrides: Vec<ShortcutPreference>) {
        let overrides = normalize_shortcut_overrides(overrides);
        if self.shortcut_overrides != overrides {
            self.shortcut_overrides = overrides;
            self.dirty = true;
        }
    }

    /// Restore every persistent preference while retaining the configured
    /// storage handle. Playback history is owned by `SessionStore` and is
    /// intentionally outside this reset.
    pub fn reset_to_defaults(&mut self) {
        let defaults = Self::default();
        let changed = self.theme != defaults.theme
            || self.accent != defaults.accent
            || self.default_volume != defaults.default_volume
            || self.default_speed != defaults.default_speed
            || self.completion_behavior != defaults.completion_behavior
            || self.hardware_decoding_enabled != defaults.hardware_decoding_enabled
            || self.window_placement.is_some()
            || !self.shortcut_overrides.is_empty();
        self.theme = defaults.theme;
        self.accent = defaults.accent;
        self.default_volume = defaults.default_volume;
        self.default_speed = defaults.default_speed;
        self.completion_behavior = defaults.completion_behavior;
        self.hardware_decoding_enabled = defaults.hardware_decoding_enabled;
        self.window_placement = None;
        self.shortcut_overrides.clear();
        if changed {
            self.dirty = true;
            // The first save keeps the previous settings in `.bak`; a second
            // commit makes reset durable even if primary recovery is needed.
            self.forget_previous_pending = true;
        }
    }

    pub fn save_if_dirty(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut content = format!(
            "{SETTINGS_FORMAT}\t{SETTINGS_VERSION}\ntheme\t{}\naccent\t{}\nvolume\t{}\nspeed\t{}\ncompletion_behavior\t{}\nhardware_decoding\t{}\n",
            self.theme.as_str(),
            self.accent.as_str(),
            self.default_volume,
            self.default_speed,
            self.completion_behavior.as_str(),
            self.hardware_decoding_enabled
        );
        if let Some(placement) = self.window_placement {
            content.push_str(&format!(
                "window_placement\t{}\t{}\t{}\t{}\n",
                placement.x, placement.y, placement.width, placement.height
            ));
        }
        for preference in &self.shortcut_overrides {
            content.push_str(&format!(
                "shortcut\t{}\t{}\t{}\n",
                preference.action_id, preference.key_code, preference.modifiers
            ));
        }
        self.persistence.save(&content)?;
        if self.forget_previous_pending {
            self.persistence.save(&content)?;
            self.forget_previous_pending = false;
        }
        self.dirty = false;
        Ok(())
    }

    #[cfg(test)]
    fn load_from(storage_path: &Path) -> std::io::Result<Self> {
        let (persistence, mut settings) = Persistence::open(storage_path, Self::parse)?;
        settings.dirty = persistence.needs_repair();
        settings.persistence = persistence;
        Ok(settings)
    }

    fn parse(content: &str) -> std::io::Result<Self> {
        let mut lines = content.lines();
        let Some(header) = lines.next() else {
            return Err(Error::new(ErrorKind::InvalidData, "пустой файл настроек"));
        };
        let mut header_fields = header.split('\t');
        let format = header_fields.next();
        let version = header_fields
            .next()
            .and_then(|value| value.parse::<u32>().ok());
        if format != Some(SETTINGS_FORMAT) || header_fields.next().is_some() || version.is_none() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "повреждён заголовок настроек",
            ));
        }
        if version != Some(SETTINGS_VERSION) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "неподдерживаемая версия настроек",
            ));
        }

        let mut settings = Self::default();
        for line in lines {
            let mut fields = line.splitn(2, '\t');
            match (fields.next(), fields.next()) {
                (Some("theme"), Some(value)) => {
                    if let Some(theme) = ThemePreference::parse(value) {
                        settings.theme = theme;
                    }
                }
                (Some("accent"), Some(value)) => {
                    if let Some(accent) = AccentPreference::parse(value) {
                        settings.accent = accent;
                    }
                }
                (Some("volume"), Some(value)) => {
                    if let Ok(volume) = value.trim().parse::<f32>()
                        && volume.is_finite()
                    {
                        settings.default_volume = volume.clamp(0.0, 1.0);
                    }
                }
                (Some("speed"), Some(value)) => {
                    if let Ok(speed) = value.trim().parse::<f32>()
                        && speed.is_finite()
                    {
                        settings.default_speed = speed.clamp(0.25, 4.0);
                    }
                }
                (Some("completion_behavior"), Some(value)) => {
                    if let Some(behavior) = CompletionBehavior::parse(value) {
                        settings.completion_behavior = behavior;
                    }
                }
                (Some("hardware_decoding"), Some(value)) => match value.trim() {
                    "true" => settings.hardware_decoding_enabled = true,
                    "false" => settings.hardware_decoding_enabled = false,
                    _ => {}
                },
                (Some("window_placement"), Some(value)) => {
                    let mut placement = value.split('\t');
                    let x = placement.next().and_then(|value| value.parse::<i32>().ok());
                    let y = placement.next().and_then(|value| value.parse::<i32>().ok());
                    let width = placement.next().and_then(|value| value.parse::<u32>().ok());
                    let height = placement.next().and_then(|value| value.parse::<u32>().ok());
                    if placement.next().is_none()
                        && let (Some(x), Some(y), Some(width), Some(height)) = (x, y, width, height)
                    {
                        settings.window_placement =
                            WindowPlacementPreference::new(x, y, width, height);
                    }
                }
                (Some("shortcut"), Some(value)) => {
                    let mut shortcut = value.split('\t');
                    let action_id = shortcut.next().and_then(|value| value.parse::<i32>().ok());
                    let key_code = shortcut.next().filter(|value| !value.is_empty());
                    let modifiers = shortcut.next().and_then(|value| value.parse::<u8>().ok());
                    if shortcut.next().is_none()
                        && let (Some(action_id), Some(key_code), Some(modifiers)) =
                            (action_id, key_code, modifiers)
                        && key_code.len() <= MAX_SHORTCUT_KEY_BYTES
                    {
                        let preference = ShortcutPreference {
                            action_id,
                            key_code: key_code.to_owned(),
                            modifiers,
                        };
                        if let Some(existing) = settings
                            .shortcut_overrides
                            .iter_mut()
                            .find(|entry| entry.action_id == action_id)
                        {
                            *existing = preference;
                        } else if settings.shortcut_overrides.len() < MAX_SHORTCUT_OVERRIDES {
                            settings.shortcut_overrides.push(preference);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(settings)
    }
}

fn normalize_shortcut_overrides(overrides: Vec<ShortcutPreference>) -> Vec<ShortcutPreference> {
    let mut normalized = Vec::<ShortcutPreference>::new();
    for preference in overrides {
        if preference.key_code.len() > MAX_SHORTCUT_KEY_BYTES {
            continue;
        }
        if let Some(existing) = normalized
            .iter_mut()
            .find(|entry| entry.action_id == preference.action_id)
        {
            *existing = preference;
        } else if normalized.len() < MAX_SHORTCUT_OVERRIDES {
            normalized.push(preference);
        }
    }
    normalized
}

fn default_storage_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .map(|directory| directory.join("Oxide").join("settings.txt"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
            .map(|directory| directory.join("oxide").join("settings.txt"))
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::tests::TestDirectory;

    use super::{
        AccentPreference, AppSettings, CompletionBehavior, DEFAULT_SPEED, DEFAULT_VOLUME,
        MAX_SHORTCUT_KEY_BYTES, MAX_SHORTCUT_OVERRIDES, ShortcutPreference, ThemePreference,
        WindowPlacementPreference,
    };

    #[test]
    fn theme_preference_round_trips_through_versioned_storage() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        let mut settings = AppSettings::load_from(&path).unwrap();
        settings.set_theme(ThemePreference::Light);
        settings.set_accent(AccentPreference::Sage);
        settings.set_default_volume(0.42);
        settings.set_default_speed(1.25);
        settings.set_completion_behavior(CompletionBehavior::RepeatPlaylist);
        settings.set_hardware_decoding_enabled(true);
        settings
            .set_window_placement(WindowPlacementPreference::new(-820, 140, 1280, 760).unwrap());
        settings.set_shortcut_overrides(vec![
            ShortcutPreference {
                action_id: 5,
                key_code: "\"KeyB\"".to_owned(),
                modifiers: 1,
            },
            ShortcutPreference {
                action_id: 6,
                key_code: "null".to_owned(),
                modifiers: 0,
            },
        ]);
        settings.save_if_dirty().expect("settings must be saved");

        let loaded = AppSettings::load_from(&path).expect("settings must be loaded");
        assert_eq!(loaded.theme(), ThemePreference::Light);
        assert_eq!(loaded.accent(), AccentPreference::Sage);
        assert_eq!(loaded.default_volume(), 0.42);
        assert_eq!(loaded.default_speed(), 1.25);
        assert_eq!(
            loaded.completion_behavior(),
            CompletionBehavior::RepeatPlaylist
        );
        assert!(loaded.hardware_decoding_enabled());
        assert_eq!(
            loaded.window_placement(),
            WindowPlacementPreference::new(-820, 140, 1280, 760)
        );
        assert_eq!(loaded.shortcut_overrides(), settings.shortcut_overrides());

        std::fs::remove_file(path).expect("temporary settings must be removed");
    }

    #[test]
    fn invalid_theme_value_falls_back_to_system() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        std::fs::write(&path, "OXIDE_SETTINGS\t1\ntheme\tneon\n").expect("fixture must be written");

        let loaded = AppSettings::load_from(&path).expect("known version must be loaded");
        assert_eq!(loaded.theme(), ThemePreference::System);

        std::fs::remove_file(path).expect("temporary settings must be removed");
    }

    #[test]
    fn malformed_setting_records_preserve_all_runtime_invariants() {
        let keys = [
            "theme",
            "accent",
            "volume",
            "speed",
            "completion_behavior",
            "hardware_decoding",
            "window_placement",
            "shortcut",
            "unknown",
            "",
        ];
        let values = [
            "",
            "NaN",
            "inf",
            "-inf",
            "-999999999999999999999",
            "999999999999999999999",
            "0",
            "1",
            "true",
            "false",
            "0\t0\t760\t520",
            "-1000001\t0\t760\t520",
            "1\t\"KeyA\"\t255",
            "1\t\"KeyB\"\t0\textra",
            "текст с пробелом",
        ];
        let mut state = 0xd1b5_4a32_d192_ed03_u64;

        for _ in 0..2_000 {
            let mut content = String::from("OXIDE_SETTINGS\t1\n");
            for _ in 0..24 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let key = keys[state as usize % keys.len()];
                state = state.rotate_left(19).wrapping_add(0x9e37_79b9_7f4a_7c15);
                let value = values[state as usize % values.len()];
                content.push_str(key);
                content.push('\t');
                content.push_str(value);
                content.push('\n');
            }

            let settings = AppSettings::parse(&content).expect("valid header must remain readable");
            assert!(settings.default_volume().is_finite());
            assert!((0.0..=1.0).contains(&settings.default_volume()));
            assert!(settings.default_speed().is_finite());
            assert!((0.25..=4.0).contains(&settings.default_speed()));
            assert!(settings.shortcut_overrides().len() <= MAX_SHORTCUT_OVERRIDES);
            assert!(settings.shortcut_overrides().iter().all(|preference| {
                !preference.key_code.is_empty()
                    && preference.key_code.len() <= MAX_SHORTCUT_KEY_BYTES
            }));
            for (index, preference) in settings.shortcut_overrides().iter().enumerate() {
                assert!(
                    settings.shortcut_overrides()[index + 1..]
                        .iter()
                        .all(|other| other.action_id != preference.action_id)
                );
            }
            if let Some(placement) = settings.window_placement() {
                assert!(placement.x.unsigned_abs() <= super::MAX_WINDOW_COORDINATE as u32);
                assert!(placement.y.unsigned_abs() <= super::MAX_WINDOW_COORDINATE as u32);
                assert!(
                    (super::MIN_WINDOW_WIDTH..=super::MAX_WINDOW_DIMENSION)
                        .contains(&placement.width)
                );
                assert!(
                    (super::MIN_WINDOW_HEIGHT..=super::MAX_WINDOW_DIMENSION)
                        .contains(&placement.height)
                );
            }
        }
    }

    #[test]
    fn legacy_amber_accent_migrates_to_iris() {
        assert_eq!(
            AccentPreference::parse("amber"),
            Some(AccentPreference::Iris)
        );
        assert_eq!(AccentPreference::Iris.as_str(), "iris");
        assert_eq!(AccentPreference::Iris.ui_index(), 1);
    }

    #[test]
    fn damaged_settings_restore_previous_theme_and_keep_shortcuts() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        let mut settings = AppSettings::load_from(&path).unwrap();
        settings.set_theme(ThemePreference::Light);
        settings.set_shortcut_overrides(vec![ShortcutPreference {
            action_id: 5,
            key_code: "null".into(),
            modifiers: 0,
        }]);
        settings.save_if_dirty().unwrap();
        settings.set_theme(ThemePreference::Dark);
        settings.save_if_dirty().unwrap();
        std::fs::write(&path, b"truncated").unwrap();
        let mut recovered = AppSettings::load_from(&path).unwrap();
        assert_eq!(recovered.theme(), ThemePreference::Light);
        assert_eq!(
            recovered.shortcut_overrides(),
            settings.shortcut_overrides()
        );
        assert!(recovered.take_storage_notice().is_some());
        assert!(recovered.dirty);
        recovered.save_if_dirty().unwrap();
        assert!(!recovered.dirty);
        assert_eq!(
            AppSettings::load_from(&path).unwrap().theme(),
            ThemePreference::Light
        );
    }

    #[test]
    fn conflict_keeps_local_changes_dirty_without_overwriting_saved_preferences() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        let mut first = AppSettings::load_from(&path).unwrap();
        let mut second = AppSettings::load_from(&path).unwrap();
        first.set_theme(ThemePreference::Light);
        first.save_if_dirty().unwrap();
        second.set_accent(AccentPreference::Sage);
        assert!(second.save_if_dirty().is_err());
        assert!(second.dirty);
        assert_eq!(second.accent(), AccentPreference::Sage);
        let saved = AppSettings::load_from(&path).unwrap();
        assert_eq!(saved.theme(), ThemePreference::Light);
        assert_eq!(saved.accent(), AccentPreference::Copper);
    }

    #[test]
    fn newer_settings_schema_is_not_downgraded_to_backup() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        let mut first = AppSettings::load_from(&path).unwrap();
        first.set_theme(ThemePreference::Light);
        first.save_if_dirty().unwrap();
        std::fs::write(&path, "OXIDE_SETTINGS\t2\ntheme\tdark\n").unwrap();
        assert_eq!(
            AppSettings::load_from(&path).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn invalid_playback_defaults_are_ignored_or_clamped() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        std::fs::write(&path, "OXIDE_SETTINGS\t1\nvolume\t5\nspeed\tnot-a-number\n")
            .expect("fixture must be written");

        let loaded = AppSettings::load_from(&path).expect("known version must be loaded");
        assert_eq!(loaded.default_volume(), 1.0);
        assert_eq!(loaded.default_speed(), DEFAULT_SPEED);
        assert_eq!(loaded.completion_behavior(), CompletionBehavior::Stop);
        assert!(!loaded.hardware_decoding_enabled());

        std::fs::remove_file(path).expect("temporary settings must be removed");
    }

    #[test]
    fn invalid_window_placements_are_ignored_without_rejecting_settings() {
        let valid =
            AppSettings::parse("OXIDE_SETTINGS\t1\nwindow_placement\t-1920\t40\t1280\t760\n")
                .unwrap();
        assert_eq!(
            valid.window_placement(),
            WindowPlacementPreference::new(-1920, 40, 1280, 760)
        );

        for value in [
            "0\t0\t759\t760",
            "0\t0\t1280\t519",
            "0\t0\t99999999\t760",
            "-2147483648\t0\t1280\t760",
            "0\t0\t1280",
            "left\t0\t1280\t760",
        ] {
            let parsed =
                AppSettings::parse(&format!("OXIDE_SETTINGS\t1\nwindow_placement\t{value}\n"))
                    .unwrap();
            assert_eq!(parsed.window_placement(), None, "fixture: {value}");
        }
    }

    #[test]
    fn shortcut_payloads_and_counts_are_bounded_while_parsing_and_setting() {
        let mut content = String::from("OXIDE_SETTINGS\t1\n");
        content.push_str(&format!(
            "shortcut\t-1\t{}\t0\n",
            "x".repeat(MAX_SHORTCUT_KEY_BYTES + 1)
        ));
        for action_id in 0..(MAX_SHORTCUT_OVERRIDES + 20) {
            content.push_str(&format!("shortcut\t{action_id}\t\"KeyA\"\t0\n"));
        }
        let parsed = AppSettings::parse(&content).expect("bounded settings must parse");
        assert_eq!(parsed.shortcut_overrides().len(), MAX_SHORTCUT_OVERRIDES);
        assert!(
            parsed
                .shortcut_overrides()
                .iter()
                .all(|preference| preference.key_code.len() <= MAX_SHORTCUT_KEY_BYTES)
        );

        let mut settings = AppSettings::default();
        settings.set_shortcut_overrides(vec![
            ShortcutPreference {
                action_id: 1,
                key_code: "\"KeyA\"".into(),
                modifiers: 0,
            },
            ShortcutPreference {
                action_id: 1,
                key_code: "\"KeyB\"".into(),
                modifiers: 0,
            },
            ShortcutPreference {
                action_id: 2,
                key_code: "x".repeat(MAX_SHORTCUT_KEY_BYTES + 1),
                modifiers: 0,
            },
        ]);
        assert_eq!(settings.shortcut_overrides().len(), 1);
        assert_eq!(settings.shortcut_overrides()[0].key_code, "\"KeyB\"");
    }

    #[test]
    fn resetting_settings_restores_every_persistent_default_and_recovery_copy() {
        let files = TestDirectory::new();
        let path = files.path("settings.txt");
        let mut settings = AppSettings::load_from(&path).unwrap();
        settings.set_theme(ThemePreference::Light);
        settings.set_accent(AccentPreference::Blue);
        settings.set_default_volume(0.2);
        settings.set_default_speed(1.75);
        settings.set_completion_behavior(CompletionBehavior::RepeatPlaylist);
        settings.set_hardware_decoding_enabled(true);
        settings.set_window_placement(WindowPlacementPreference::new(100, 200, 900, 600).unwrap());
        settings.set_shortcut_overrides(vec![ShortcutPreference {
            action_id: 5,
            key_code: "null".into(),
            modifiers: 0,
        }]);
        settings.save_if_dirty().unwrap();

        settings.reset_to_defaults();
        settings.save_if_dirty().unwrap();

        let assert_defaults = |loaded: &AppSettings| {
            assert_eq!(loaded.theme(), ThemePreference::System);
            assert_eq!(loaded.accent(), AccentPreference::Copper);
            assert_eq!(loaded.default_volume(), DEFAULT_VOLUME);
            assert_eq!(loaded.default_speed(), DEFAULT_SPEED);
            assert_eq!(loaded.completion_behavior(), CompletionBehavior::Stop);
            assert!(!loaded.hardware_decoding_enabled());
            assert_eq!(loaded.window_placement(), None);
            assert!(loaded.shortcut_overrides().is_empty());
        };
        assert_defaults(&AppSettings::load_from(&path).unwrap());

        std::fs::write(&path, b"broken").unwrap();
        let mut recovered = AppSettings::load_from(&path).unwrap();
        assert_defaults(&recovered);
        assert!(recovered.take_storage_notice().is_some());
    }
}
