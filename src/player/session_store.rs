use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

const STATE_HEADER: &str = "OXIDE_PLAYBACK_STATE_V1";
const MAX_RECENT_FILES: usize = 10;
const RESUME_MIN_SECONDS: f64 = 5.0;
const RESUME_END_GUARD_SECONDS: f64 = 10.0;

#[derive(Debug, Default)]
pub struct SessionStore {
    recent: Vec<PathBuf>,
    positions: HashMap<PathBuf, f64>,
    storage_path: Option<PathBuf>,
    dirty: bool,
}

impl SessionStore {
    pub fn load_default() -> Self {
        let Some(storage_path) = default_storage_path() else {
            return Self::default();
        };
        Self::load_from(&storage_path).unwrap_or_else(|_| Self {
            storage_path: Some(storage_path),
            ..Self::default()
        })
    }

    fn load_from(storage_path: &Path) -> std::io::Result<Self> {
        let content = match fs::read_to_string(storage_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    storage_path: Some(storage_path.to_owned()),
                    ..Self::default()
                });
            }
            Err(error) => return Err(error),
        };
        let mut lines = content.lines();
        if lines.next() != Some(STATE_HEADER) {
            return Ok(Self {
                storage_path: Some(storage_path.to_owned()),
                ..Self::default()
            });
        }

        let mut store = Self {
            storage_path: Some(storage_path.to_owned()),
            ..Self::default()
        };
        for line in lines {
            let mut fields = line.split('\t');
            match fields.next() {
                Some("recent") => {
                    if let Some(path) = fields.next().and_then(decode_path) {
                        store.recent.push(path);
                    }
                }
                Some("position") => {
                    let seconds = fields.next().and_then(|value| value.parse::<f64>().ok());
                    let path = fields.next().and_then(decode_path);
                    if let (Some(seconds), Some(path)) = (seconds, path)
                        && seconds.is_finite()
                        && seconds >= RESUME_MIN_SECONDS
                    {
                        store.positions.insert(path, seconds);
                    }
                }
                _ => {}
            }
        }
        store.recent.truncate(MAX_RECENT_FILES);
        Ok(store)
    }

    pub fn recent(&self) -> &[PathBuf] {
        &self.recent
    }

    pub fn record_open(&mut self, path: &Path) {
        let path = path.to_owned();
        self.recent.retain(|existing| existing != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT_FILES);
        self.dirty = true;
    }

    pub fn record_position(&mut self, path: &Path, seconds: f64, duration: f64) {
        if !seconds.is_finite() || seconds < RESUME_MIN_SECONDS {
            if self.positions.remove(path).is_some() {
                self.dirty = true;
            }
            return;
        }
        if duration.is_finite() && duration > 0.0 && duration - seconds <= RESUME_END_GUARD_SECONDS
        {
            self.clear_position(path);
            return;
        }

        let previous = self.positions.insert(path.to_owned(), seconds);
        if previous != Some(seconds) {
            self.dirty = true;
        }
    }

    pub fn clear_position(&mut self, path: &Path) {
        if self.positions.remove(path).is_some() {
            self.dirty = true;
        }
    }

    pub fn resume_position(&self, path: &Path, duration: f64) -> Option<f64> {
        let position = self.positions.get(path).copied()?;
        (position >= RESUME_MIN_SECONDS
            && (!duration.is_finite()
                || duration <= 0.0
                || duration - position > RESUME_END_GUARD_SECONDS))
            .then_some(position)
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

        let mut content = String::from(STATE_HEADER);
        content.push('\n');
        for path in &self.recent {
            let _ = writeln!(content, "recent\t{}", encode_path(path));
        }
        for (path, seconds) in &self.positions {
            let _ = writeln!(content, "position\t{seconds:.3}\t{}", encode_path(path));
        }
        fs::write(storage_path, content)?;
        self.dirty = false;
        Ok(())
    }
}

fn default_storage_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .map(|directory| directory.join("Oxide").join("playback-state.txt"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("state"))
            })
            .map(|directory| directory.join("oxide").join("playback-state.txt"))
    }
}

fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .as_bytes()
        .iter()
        .fold(String::new(), |mut encoded, byte| {
            let _ = write!(encoded, "{byte:02x}");
            encoded
        })
}

fn decode_path(value: &str) -> Option<PathBuf> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::{SessionStore, decode_path, encode_path};
    use std::path::{Path, PathBuf};

    #[test]
    fn path_encoding_round_trips_unicode_without_delimiter_escaping() {
        let path = Path::new(r"C:\Музыка\трек с пробелом.flac");
        assert_eq!(decode_path(&encode_path(path)), Some(path.to_owned()));
    }

    #[test]
    fn recent_files_are_unique_and_newest_first() {
        let mut store = SessionStore::default();
        store.record_open(Path::new("one.mp4"));
        store.record_open(Path::new("two.mp4"));
        store.record_open(Path::new("one.mp4"));
        assert_eq!(
            store.recent,
            [PathBuf::from("one.mp4"), PathBuf::from("two.mp4")]
        );
    }

    #[test]
    fn resume_position_ignores_opening_and_near_end_positions() {
        let mut store = SessionStore::default();
        let path = Path::new("movie.mkv");
        store.record_position(path, 3.0, 100.0);
        assert_eq!(store.resume_position(path, 100.0), None);

        store.record_position(path, 45.0, 100.0);
        assert_eq!(store.resume_position(path, 100.0), Some(45.0));

        store.record_position(path, 95.0, 100.0);
        assert_eq!(store.resume_position(path, 100.0), None);
    }
}
