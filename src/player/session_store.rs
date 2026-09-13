use crate::storage::Persistence;
use std::{
    collections::HashMap,
    fmt::Write as _,
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
    persistence: Persistence,
    last_save_error: Option<String>,
    dirty: bool,
}

impl SessionStore {
    pub fn load_default() -> Self {
        let (persistence, mut store) = Persistence::load(default_storage_path(), Self::parse);
        store.dirty = persistence.needs_repair();
        store.persistence = persistence;
        store
    }

    #[cfg(test)]
    fn load_from(storage_path: &Path) -> std::io::Result<Self> {
        let (persistence, mut store) = Persistence::open(storage_path, Self::parse)?;
        store.dirty = persistence.needs_repair();
        store.persistence = persistence;
        Ok(store)
    }

    fn parse(content: &str) -> std::io::Result<Self> {
        let mut lines = content.lines();
        let header = lines.next().unwrap_or_default();
        if header != STATE_HEADER {
            return Err(std::io::Error::new(
                if header.starts_with("OXIDE_PLAYBACK_STATE_V") {
                    std::io::ErrorKind::Unsupported
                } else {
                    std::io::ErrorKind::InvalidData
                },
                "Неизвестный или повреждённый формат истории воспроизведения",
            ));
        }

        let mut store = Self::default();
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

    pub fn take_storage_notice(&mut self) -> Option<String> {
        self.persistence.take_notice()
    }

    /// Surface an error once, not on every periodic save. Keep dirty for retry.
    pub fn save_and_take_error(&mut self) -> Option<String> {
        match self.save_if_dirty() {
            Ok(()) => {
                self.last_save_error = None;
                None
            }
            Err(error) => {
                let message = format!("Не удалось сохранить историю и позиции: {error}");
                if self.last_save_error.as_ref() == Some(&message) {
                    return None;
                }
                self.last_save_error = Some(message.clone());
                Some(message)
            }
        }
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
        let mut content = String::from(STATE_HEADER);
        content.push('\n');
        for path in &self.recent {
            let _ = writeln!(content, "recent\t{}", encode_path(path));
        }
        for (path, seconds) in &self.positions {
            let _ = writeln!(content, "position\t{seconds:.3}\t{}", encode_path(path));
        }
        self.persistence.save(&content)?;
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
    use crate::storage::tests::TestDirectory;
    use std::path::{Path, PathBuf};

    #[test]
    fn path_encoding_round_trips_unicode_without_delimiter_escaping() {
        let path = Path::new(r"C:\Музыка\трек с пробелом.flac");
        assert_eq!(decode_path(&encode_path(path)), Some(path.to_owned()));
    }

    #[test]
    fn history_and_resume_position_round_trip_and_recover_together() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let track = Path::new("Музыка/мой трек.flac");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(track);
        store.record_position(track, 42.5, 100.0);
        store.save_if_dirty().unwrap();
        store.record_position(track, 60.0, 100.0);
        store.save_if_dirty().unwrap();
        assert_eq!(
            SessionStore::load_from(&path)
                .unwrap()
                .resume_position(track, 100.0),
            Some(60.0)
        );
        std::fs::write(&path, b"broken").unwrap();
        let mut recovered = SessionStore::load_from(&path).unwrap();
        assert_eq!(recovered.recent(), &[track.to_owned()]);
        assert_eq!(recovered.resume_position(track, 100.0), Some(42.5));
        assert!(recovered.take_storage_notice().is_some());
        recovered.save_if_dirty().unwrap();
        assert!(!recovered.dirty);
    }

    #[test]
    fn failed_session_save_is_reported_once_and_can_be_retried() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let lock_path = files.path("playback-state.txt.lock");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(Path::new("movie.mkv"));
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(lock_path)
            .unwrap();
        lock.try_lock().unwrap();
        assert!(store.save_and_take_error().is_some());
        assert!(store.dirty);
        assert!(store.save_and_take_error().is_none());
        drop(lock);
        assert!(store.save_and_take_error().is_none());
        assert!(!store.dirty);
        assert_eq!(
            SessionStore::load_from(&path).unwrap().recent(),
            &[PathBuf::from("movie.mkv")]
        );
    }

    #[test]
    fn newer_history_schema_is_not_silently_reset() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        std::fs::write(&path, "OXIDE_PLAYBACK_STATE_V2\n").unwrap();
        assert_eq!(
            SessionStore::load_from(&path).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
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
