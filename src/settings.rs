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
    Amber,
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
            1 => Some(Self::Amber),
            2 => Some(Self::Sage),
            3 => Some(Self::Blue),
            _ => None,
        }
    }

    pub const fn ui_index(self) -> i32 {
        match self {
            Self::Copper => 0,
            Self::Amber => 1,
            Self::Sage => 2,
            Self::Blue => 3,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Copper => "copper",
            Self::Amber => "amber",
            Self::Sage => "sage",
            Self::Blue => "blue",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "copper" => Some(Self::Copper),
            "amber" => Some(Self::Amber),
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
    shortcut_overrides: Vec<ShortcutPreference>,
    persistence: Persistence,
    dirty: bool,
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
            shortcut_overrides: Vec::new(),
            persistence: Persistence::default(),
            dirty: false,
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

    pub fn set_shortcut_overrides(&mut self, overrides: Vec<ShortcutPreference>) {
        if self.shortcut_overrides != overrides {
            self.shortcut_overrides = overrides;
            self.dirty = true;
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
        for preference in &self.shortcut_overrides {
            content.push_str(&format!(
                "shortcut\t{}\t{}\t{}\n",
                preference.action_id, preference.key_code, preference.modifiers
            ));
        }
        self.persistence.save(&content)?;
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
                (Some("shortcut"), Some(value)) => {
                    let mut shortcut = value.split('\t');
                    let action_id = shortcut.next().and_then(|value| value.parse::<i32>().ok());
                    let key_code = shortcut.next().filter(|value| !value.is_empty());
                    let modifiers = shortcut.next().and_then(|value| value.parse::<u8>().ok());
                    if shortcut.next().is_none()
                        && let (Some(action_id), Some(key_code), Some(modifiers)) =
                            (action_id, key_code, modifiers)
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
                        } else {
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
        AccentPreference, AppSettings, CompletionBehavior, DEFAULT_SPEED, ShortcutPreference,
        ThemePreference,
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
}
