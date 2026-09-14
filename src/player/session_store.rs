use crate::storage::Persistence;
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

mod positions;
use positions::{MAX_PATH_BYTES, Positions, path_bytes};

const STATE_HEADER: &str = "OXIDE_PLAYBACK_STATE_V2";
const LEGACY_HEADER: &str = "OXIDE_PLAYBACK_STATE_V1";
const MAX_RECENT_FILES: usize = 10;
const MAX_RECENT_PATH_BYTES: usize = 256 * 1024;
const RESUME_MIN_SECONDS: f64 = 5.0;
const RESUME_END_GUARD_SECONDS: f64 = 10.0;

#[derive(Debug, Default)]
pub struct SessionStore {
    recent: Vec<PathBuf>,
    positions: Positions,
    persistence: Persistence,
    last_save_error: Option<String>,
    dirty: bool,
}

impl SessionStore {
    pub fn load_default() -> Self {
        let (persistence, mut store) = Persistence::load(default_storage_path(), Self::parse);
        store.dirty |= persistence.needs_repair();
        store.persistence = persistence;
        store
    }

    #[cfg(test)]
    fn load_from(storage_path: &Path) -> std::io::Result<Self> {
        let (persistence, mut store) = Persistence::open(storage_path, Self::parse)?;
        store.dirty |= persistence.needs_repair();
        store.persistence = persistence;
        Ok(store)
    }

    fn parse(content: &str) -> std::io::Result<Self> {
        let mut lines = content.lines();
        let header = lines.next().unwrap_or_default();
        let legacy = header == LEGACY_HEADER;
        if header != STATE_HEADER && !legacy {
            return Err(std::io::Error::new(
                if header.starts_with("OXIDE_PLAYBACK_STATE_V") {
                    std::io::ErrorKind::Unsupported
                } else {
                    std::io::ErrorKind::InvalidData
                },
                "Неизвестный или повреждённый формат истории воспроизведения",
            ));
        }

        let mut store = Self {
            dirty: legacy,
            ..Self::default()
        };
        // Read the small recent list first, even if legacy records are reordered.
        // Never accumulate an unbounded vector and truncate it afterwards.
        for line in lines.clone() {
            if let Some(encoded) = line.strip_prefix("recent\t") {
                if let Some(path) = decode_path(encoded) {
                    if store.recent.contains(&path) {
                        store.dirty = true;
                        continue;
                    }
                    let bytes = path_bytes(&path).expect("decoded path");
                    if store.recent.len() < MAX_RECENT_FILES
                        && store
                            .recent
                            .iter()
                            .filter_map(|path| path_bytes(path))
                            .sum::<usize>()
                            + bytes
                            <= MAX_RECENT_PATH_BYTES
                    {
                        store.recent.push(path);
                    } else {
                        store.dirty = true;
                    }
                } else {
                    store.dirty = true;
                }
            }
        }
        for line in lines {
            let mut fields = line.split('\t');
            if fields.next() == Some("position") {
                let seconds = fields.next().and_then(|value| value.parse::<f64>().ok());
                let path = fields.next().and_then(decode_path);
                if let (Some(seconds), Some(path)) = (seconds, path)
                    && seconds.is_finite()
                    && seconds >= RESUME_MIN_SECONDS
                {
                    let protected = if legacy { store.recent.as_slice() } else { &[] };
                    store.dirty |= store.positions.record(&path, seconds, protected).discarded;
                } else {
                    store.dirty = true;
                }
            }
        }
        if legacy {
            for path in store.recent.iter().rev() {
                store.positions.touch(path);
            }
        }
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
        if path_bytes(path).is_none() {
            return;
        }
        self.positions.touch(path);
        let path = path.to_owned();
        self.recent.retain(|existing| existing != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT_FILES);
        while self
            .recent
            .iter()
            .filter_map(|path| path_bytes(path))
            .sum::<usize>()
            > MAX_RECENT_PATH_BYTES
        {
            self.recent.pop();
        }
        self.dirty = true;
    }

    pub fn record_position(&mut self, path: &Path, seconds: f64, duration: f64) {
        if !seconds.is_finite() || seconds < RESUME_MIN_SECONDS {
            if self.positions.remove(path) {
                self.dirty = true;
            }
            return;
        }
        if duration.is_finite() && duration > 0.0 && duration - seconds <= RESUME_END_GUARD_SECONDS
        {
            self.clear_position(path);
            return;
        }

        self.dirty |= self.positions.record(path, seconds, &[]).changed;
    }

    pub fn clear_position(&mut self, path: &Path) {
        if self.positions.remove(path) {
            self.dirty = true;
        }
    }

    pub fn resume_position(&self, path: &Path, duration: f64) -> Option<f64> {
        let position = self.positions.get(path)?;
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
        // V2 persists oldest-to-newest order, without wall-clock timestamps.
        for (path, seconds) in self.positions.iter() {
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
    if value.is_empty() || value.len() > MAX_PATH_BYTES * 2 || !value.len().is_multiple_of(2) {
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
    let path = String::from_utf8(bytes).ok().map(PathBuf::from)?;
    path_bytes(&path)?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::{SessionStore, decode_path, encode_path};
    use crate::storage::tests::TestDirectory;
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    #[test]
    fn legacy_history_migrates_without_losing_positions_and_keeps_backup() {
        let files = TestDirectory::new();
        let path = files.path("state.txt");
        let track = Path::new("старый трек.mp3");
        let old = format!(
            "OXIDE_PLAYBACK_STATE_V1\nrecent\t{}\nposition\t42.500\t{}\n",
            encode_path(track),
            encode_path(track)
        );
        std::fs::write(&path, &old).unwrap();
        let mut store = SessionStore::load_from(&path).unwrap();
        assert!(store.dirty);
        assert_eq!(store.resume_position(track, 100.0), Some(42.5));
        store.save_if_dirty().unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains(super::STATE_HEADER)
        );
        assert!(
            std::fs::read_to_string(files.path("state.txt.bak"))
                .unwrap()
                .contains(&old)
        );
        let loaded = SessionStore::load_from(&path).unwrap();
        assert_eq!(loaded.resume_position(track, 100.0), Some(42.5));
        assert!(!loaded.dirty);
    }

    #[test]
    fn position_recency_survives_restart_and_controls_eviction() {
        let files = TestDirectory::new();
        let path = files.path("state.txt");
        let mut store = SessionStore::load_from(&path).unwrap();
        for index in 0..super::positions::MAX_POSITIONS {
            store.record_position(Path::new(&format!("{index}.mp4")), 30.0, 100.0);
        }
        store.record_open(Path::new("0.mp4"));
        store.save_if_dirty().unwrap();
        let mut loaded = SessionStore::load_from(&path).unwrap();
        loaded.record_position(Path::new("new.mp4"), 40.0, 100.0);
        assert_eq!(
            loaded.resume_position(Path::new("0.mp4"), 100.0),
            Some(30.0)
        );
        assert_eq!(loaded.resume_position(Path::new("1.mp4"), 100.0), None);
        assert_eq!(
            loaded.positions.iter().count(),
            super::positions::MAX_POSITIONS
        );
    }

    #[test]
    fn oversized_legacy_history_keeps_recent_positions_even_when_recent_lines_come_last() {
        let started = std::time::Instant::now();
        let mut content = String::from("OXIDE_PLAYBACK_STATE_V1\n");
        let keep = Path::new("old-but-recent.mp4");
        writeln!(content, "position\t30\t{}", encode_path(keep)).unwrap();
        for index in 0..20_000 {
            writeln!(
                content,
                "position\t40\t{}",
                encode_path(Path::new(&format!("{index}.mp4")))
            )
            .unwrap();
        }
        writeln!(content, "recent\t{}", encode_path(keep)).unwrap();
        let store = SessionStore::parse(&content).unwrap();
        assert!(store.dirty);
        assert_eq!(store.resume_position(keep, 100.0), Some(30.0));
        assert_eq!(
            store.resume_position(Path::new("19999.mp4"), 100.0),
            Some(40.0)
        );
        assert_eq!(
            store.positions.iter().count(),
            super::positions::MAX_POSITIONS
        );
        assert_eq!(store.positions.iter().last().unwrap().0, keep);
        eprintln!(
            "legacy retention: 20001 positions reduced to 1000, build+parse {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn encoded_path_limits_reject_large_or_invalid_paths_before_decoding() {
        assert!(decode_path("").is_none());
        assert!(decode_path("a").is_none());
        assert!(decode_path("gg").is_none());
        assert!(decode_path("ff").is_none());
        let oversized = "61".repeat(super::MAX_PATH_BYTES + 1);
        assert!(decode_path(&oversized).is_none());
        let maximum = "x".repeat(super::MAX_PATH_BYTES);
        assert_eq!(
            decode_path(&encode_path(Path::new(&maximum))).unwrap(),
            Path::new(&maximum)
        );
    }

    #[test]
    fn recent_records_are_deduplicated_and_bounded_while_parsing() {
        let mut content = format!("{}\n", super::STATE_HEADER);
        for index in 0..20_000 {
            let path = encode_path(Path::new(&format!("{}.mp4", index / 2)));
            writeln!(content, "recent\t{path}").unwrap();
        }
        let store = SessionStore::parse(&content).unwrap();
        assert!(store.dirty);
        assert_eq!(store.recent.len(), super::MAX_RECENT_FILES);
        assert_eq!(store.recent[0], Path::new("0.mp4"));
        assert_eq!(store.recent[9], Path::new("9.mp4"));
    }

    #[test]
    fn long_path_budgets_keep_serialized_state_below_storage_limit() {
        let files = TestDirectory::new();
        let path = files.path("state.txt");
        let mut store = SessionStore::load_from(&path).unwrap();
        // Synthetic paths only: no attempt to open or create these media files.
        for index in 0..20 {
            let text = format!("{index:02}{}", "x".repeat(super::MAX_PATH_BYTES - 2));
            store.record_open(Path::new(&text));
            store.record_position(Path::new(&text), 30.0, 100.0);
        }
        assert!(
            store
                .recent
                .iter()
                .filter_map(|path| super::path_bytes(path))
                .sum::<usize>()
                <= super::MAX_RECENT_PATH_BYTES
        );
        store.save_if_dirty().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 3 * 1024 * 1024);
        let loaded = SessionStore::load_from(&path).unwrap();
        assert_eq!(
            loaded.positions.iter().count(),
            store.positions.iter().count()
        );
        assert_eq!(loaded.recent(), store.recent());
        assert!(!loaded.dirty);
    }

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
        std::fs::write(&path, "OXIDE_PLAYBACK_STATE_V3\n").unwrap();
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
