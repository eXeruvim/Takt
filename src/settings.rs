use std::{
    fs,
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
};

const SETTINGS_FORMAT: &str = "OXIDE_SETTINGS";
const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
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
    storage_path: Option<PathBuf>,
    dirty: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            storage_path: None,
            dirty: false,
        }
    }
}

impl AppSettings {
    pub fn load_default() -> Self {
        let Some(storage_path) = default_storage_path() else {
            return Self::default();
        };
        Self::load_from(&storage_path).unwrap_or_else(|_| Self {
            storage_path: Some(storage_path),
            ..Self::default()
        })
    }

    pub const fn theme(&self) -> ThemePreference {
        self.theme
    }

    pub fn set_theme(&mut self, theme: ThemePreference) {
        if self.theme != theme {
            self.theme = theme;
            self.dirty = true;
        }
    }

    pub fn save_if_dirty(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let Some(storage_path) = self.storage_path.as_ref() else {
            self.dirty = false;
            return Ok(());
        };
        if let Some(parent) = storage_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = format!(
            "{SETTINGS_FORMAT}\t{SETTINGS_VERSION}\ntheme\t{}\n",
            self.theme.as_str()
        );
        fs::write(storage_path, content)?;
        self.dirty = false;
        Ok(())
    }

    fn load_from(storage_path: &Path) -> std::io::Result<Self> {
        let content = match fs::read_to_string(storage_path) {
            Ok(content) => content,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(Self {
                    storage_path: Some(storage_path.to_owned()),
                    ..Self::default()
                });
            }
            Err(error) => return Err(error),
        };

        let mut lines = content.lines();
        let Some(header) = lines.next() else {
            return Err(Error::new(ErrorKind::InvalidData, "пустой файл настроек"));
        };
        let mut header_fields = header.split('\t');
        let format = header_fields.next();
        let version = header_fields
            .next()
            .and_then(|value| value.parse::<u32>().ok());
        if format != Some(SETTINGS_FORMAT) || version != Some(SETTINGS_VERSION) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "неподдерживаемая версия настроек",
            ));
        }

        let mut settings = Self {
            storage_path: Some(storage_path.to_owned()),
            ..Self::default()
        };
        for line in lines {
            let mut fields = line.splitn(2, '\t');
            if let (Some("theme"), Some(value)) = (fields.next(), fields.next())
                && let Some(theme) = ThemePreference::parse(value)
            {
                settings.theme = theme;
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{AppSettings, ThemePreference};

    fn temporary_settings_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("oxide-settings-{nonce}.txt"))
    }

    #[test]
    fn theme_preference_round_trips_through_versioned_storage() {
        let path = temporary_settings_path();
        let mut settings = AppSettings {
            storage_path: Some(path.clone()),
            ..AppSettings::default()
        };
        settings.set_theme(ThemePreference::Light);
        settings.save_if_dirty().expect("settings must be saved");

        let loaded = AppSettings::load_from(&path).expect("settings must be loaded");
        assert_eq!(loaded.theme(), ThemePreference::Light);

        std::fs::remove_file(path).expect("temporary settings must be removed");
    }

    #[test]
    fn invalid_theme_value_falls_back_to_system() {
        let path = temporary_settings_path();
        std::fs::write(&path, "OXIDE_SETTINGS\t1\ntheme\tneon\n").expect("fixture must be written");

        let loaded = AppSettings::load_from(&path).expect("known version must be loaded");
        assert_eq!(loaded.theme(), ThemePreference::System);

        std::fs::remove_file(path).expect("temporary settings must be removed");
    }
}
